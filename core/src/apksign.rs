//! Signing with APK Signature Scheme v2 + v3 (RSA-2048, PKCS#1 v1.5, SHA-256), key and
//! self-signed certificate generation, and verification of v1/v2/v3 signatures.
//! Format: https://source.android.com/docs/security/features/apksigning/v2 (and /v3)

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rsa::pkcs1v15::{Signature, SigningKey, VerifyingKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey};
use rsa::signature::{SignatureEncoding, Signer, Verifier};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Digest, Sha256};

use crate::sign::{V2_ID, V3_ID};
use crate::{Error, Result};

/// RSASSA-PKCS1-v1_5 with SHA2-256, chunked SHA2-256 content digest
const RSA_SHA256: u32 = 0x0103;
/// v2 attribute telling verifiers a v3 signature exists, so v3 can't be stripped
const STRIPPING_PROTECTION: u32 = 0xbeef_f00d;
const PADDING_ID: u32 = 0x4272_6577;
const V3_MIN_SDK: u32 = 28;
const CHUNK: usize = 1 << 20;

// ---------- DER writing ----------

mod v1;

fn tlv(tag: u8, c: &[u8]) -> Vec<u8> {
    let mut o = vec![tag];
    match c.len() {
        n @ 0..0x80 => o.push(n as u8),
        n => {
            let bytes: Vec<u8> = n
                .to_be_bytes()
                .into_iter()
                .skip_while(|&b| b == 0)
                .collect();
            o.push(0x80 | bytes.len() as u8);
            o.extend(bytes);
        }
    }
    o.extend_from_slice(c);
    o
}

fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

fn der_oid(s: &str) -> Vec<u8> {
    let n: Vec<u64> = s.split('.').map(|x| x.parse().unwrap()).collect();
    let mut c = vec![(n[0] * 40 + n[1]) as u8];
    for &v in &n[2..] {
        let mut b = vec![(v & 0x7f) as u8];
        let mut v = v >> 7;
        while v > 0 {
            b.push(0x80 | (v & 0x7f) as u8);
            v >>= 7;
        }
        b.reverse();
        c.extend(b);
    }
    tlv(0x06, &c)
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// UTCTime before 2050, GeneralizedTime from 2050 (as X.509 requires).
fn der_time(unix: i64) -> Vec<u8> {
    let (y, m, d) = civil(unix.div_euclid(86_400));
    let s = unix.rem_euclid(86_400);
    let hms = format!("{:02}{:02}{:02}Z", s / 3600, s / 60 % 60, s % 60);
    if y < 2050 {
        tlv(0x17, format!("{:02}{m:02}{d:02}{hms}", y % 100).as_bytes())
    } else {
        tlv(0x18, format!("{y:04}{m:02}{d:02}{hms}").as_bytes())
    }
}

fn sha256_rsa_alg() -> Vec<u8> {
    seq(&[der_oid("1.2.840.113549.1.1.11"), vec![0x05, 0x00]])
}

// ---------- keys ----------

pub struct Key {
    pub private: RsaPrivateKey,
    /// X.509 certificate, DER
    pub cert: Vec<u8>,
}

/// New RSA-2048 key with a self-signed certificate for `name`, valid 30 years.
pub fn generate(name: &str) -> Result<Key> {
    let mut rng = rand::thread_rng();
    let private = RsaPrivateKey::new(&mut rng, 2048)?;
    let spki = private
        .to_public_key()
        .to_public_key_der()
        .map_err(|e| Error::signature(format!("public key: {e}")))?
        .as_bytes()
        .to_vec();
    let mut serial: [u8; 16] = rand::random();
    serial[0] &= 0x7f;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let dn = seq(&[tlv(
        0x31,
        &seq(&[der_oid("2.5.4.3"), tlv(0x0c, name.as_bytes())]),
    )]);
    let tbs = seq(&[
        tlv(0xa0, &tlv(0x02, &[2])),
        tlv(0x02, &serial),
        sha256_rsa_alg(),
        dn.clone(),
        seq(&[der_time(now - 86_400), der_time(now + 30 * 365 * 86_400)]),
        dn,
        spki,
    ]);
    let sig = SigningKey::<Sha256>::new(private.clone())
        .sign(&tbs)
        .to_vec();
    let cert = seq(&[
        tbs,
        sha256_rsa_alg(),
        tlv(0x03, &[&[0u8][..], &sig].concat()),
    ]);
    Ok(Key { private, cert })
}

/// `-----BEGIN ...-----` base64 body, or the bytes as they are (DER).
fn pem_or_der(d: &[u8]) -> Vec<u8> {
    let Ok(t) = std::str::from_utf8(d) else {
        return d.to_vec();
    };
    if !t.contains("-----BEGIN") {
        return d.to_vec();
    }
    let body: String = t
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in body.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Load a PKCS#8 private key and its certificate (PEM or DER).
pub fn load(key_file: &Path, cert_file: &Path) -> Result<Key> {
    let k = pem_or_der(&std::fs::read(key_file).map_err(Error::io(key_file))?);
    let private = RsaPrivateKey::from_pkcs8_der(&k).map_err(|source| Error::Key {
        path: key_file.into(),
        source,
    })?;
    let cert = pem_or_der(&std::fs::read(cert_file).map_err(Error::io(cert_file))?);
    crate::sign::parse_cert(&cert).ok_or_else(|| Error::Certificate {
        path: cert_file.into(),
    })?;
    Ok(Key { private, cert })
}

/// The signing key of a JKS or PKCS#12 keystore: the entry `alias` (or the only one), its
/// password `key_password` (default: the store password). RSA keys only.
pub fn load_keystore(
    path: &Path,
    store_password: &str,
    key_password: Option<&str>,
    alias: Option<&str>,
) -> Result<Key> {
    let data = std::fs::read(path).map_err(Error::io(path))?;
    let e = crate::keystore::pick(
        crate::keystore::open(&data, store_password, key_password)?,
        alias,
    )?;
    let private = RsaPrivateKey::from_pkcs8_der(&e.key).map_err(|_| {
        Error::Keystore(format!(
            "key {} is not an RSA key; apkscope signs with RSA keys only",
            e.alias
        ))
    })?;
    let cert = e
        .chain
        .into_iter()
        .next()
        .ok_or_else(|| Error::Keystore(format!("key {} has no certificate", e.alias)))?;
    crate::sign::parse_cert(&cert)
        .ok_or_else(|| Error::Keystore(format!("key {}: certificate is not X.509", e.alias)))?;
    Ok(Key { private, cert })
}

/// apkscope's own key: `%APPDATA%\apkscope\keys\apkscope.pk8` + `apkscope.crt`, created on first use.
pub fn default_key() -> Result<(Key, PathBuf)> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        // a key in a temp folder would change after a reboot, breaking app updates
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("apkscope").join("keys");
    let (k, c) = (dir.join("apkscope.pk8"), dir.join("apkscope.crt"));
    if k.exists() && c.exists() {
        return load(&k, &c).map(|key| (key, dir));
    }
    let key = generate("apkscope")?;
    std::fs::create_dir_all(&dir).map_err(Error::io(&dir))?;
    let pk8 = key.private.to_pkcs8_der().map_err(|source| Error::Key {
        path: k.clone(),
        source,
    })?;
    std::fs::write(&k, pk8.as_bytes()).map_err(Error::io(&k))?;
    std::fs::write(&c, &key.cert).map_err(Error::io(&c))?;
    Ok((key, dir))
}

// ---------- signing ----------

/// u32-length-prefixed
fn lp(d: &[u8]) -> Vec<u8> {
    [&(d.len() as u32).to_le_bytes()[..], d].concat()
}

/// The digests of an APK's 1 MB chunks, taken as its sections are written. A section's last
/// chunk is as short as what is left of it.
#[derive(Default)]
pub(crate) struct Chunks {
    /// The start of a chunk that is not whole yet.
    pending: Vec<u8>,
    digests: Vec<sha2::digest::Output<Sha256>>,
}

impl Chunks {
    /// More of the section being written.
    pub(crate) fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let n = (CHUNK - self.pending.len()).min(data.len());
            match n == CHUNK {
                true => self.digests.push(chunk(&data[..n])),
                false => {
                    self.pending.extend_from_slice(&data[..n]);
                    if self.pending.len() == CHUNK {
                        self.end();
                    }
                }
            }
            data = &data[n..];
        }
    }

    /// The section being written ends here.
    pub(crate) fn end(&mut self) {
        if !self.pending.is_empty() {
            self.digests.push(chunk(&self.pending));
            self.pending.clear();
        }
    }

    /// The digest of the sections written, each ended.
    fn digest(&self) -> Vec<u8> {
        let mut top = Sha256::new();
        top.update([0x5a]);
        top.update((self.digests.len() as u32).to_le_bytes());
        for c in &self.digests {
            top.update(c);
        }
        top.finalize().to_vec()
    }
}

fn chunk(c: &[u8]) -> sha2::digest::Output<Sha256> {
    let mut h = Sha256::new();
    h.update([0xa5]);
    h.update((c.len() as u32).to_le_bytes());
    h.update(c);
    h.finalize()
}

/// Chunked SHA-256 over the three APK sections the signature covers.
fn content_digest(sections: &[&[u8]]) -> Vec<u8> {
    let mut chunks = Chunks::default();
    for s in sections {
        chunks.update(s);
        chunks.end();
    }
    chunks.digest()
}

/// End-of-central-directory start (no comment support beyond what the scan finds).
fn eocd_at(apk: &[u8]) -> Result<usize> {
    (0..apk.len().saturating_sub(21))
        .rev()
        .take(65_557)
        .find(|&i| apk[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| Error::signature("no end of central directory"))
}

/// Sign the APK in `apk`, where it is, with v2 + v3: one without a signing block, as `rebuild`
/// writes one. Its entries are read once for their digests, a chunk at a time, and the signing
/// block is written after them, with the central directory moved behind it. How many bytes
/// the APK is then.
pub fn sign<F: Read + Write + Seek>(apk: &mut F, key: &Key) -> Result<u64> {
    sign_with(apk, key, false)
}

/// [`sign`], and with `v1` a JAR signature first (`META-INF/MANIFEST.MF`, `CERT.SF`,
/// `CERT.RSA`), which the v2/v3 block then covers: what Android 5-6 (API < 24) check. It
/// digests every entry's contents, so it is only worth its time for apps that run there.
pub fn sign_with<F: Read + Write + Seek>(apk: &mut F, key: &Key, v1: bool) -> Result<u64> {
    let read = |e| Error::from(zip::result::ZipError::Io(e));
    // the end record, the last thing in the file but for its comment, says where the central
    // directory is
    let len = apk.seek(SeekFrom::End(0)).map_err(read)?;
    let mut tail = vec![0; len.min(65_557) as usize];
    apk.seek(SeekFrom::Start(len - tail.len() as u64))
        .map_err(read)?;
    apk.read_exact(&mut tail).map_err(read)?;
    let mut end = tail.split_off(eocd_at(&tail)?);
    let size = u32::from_le_bytes(end[12..16].try_into().unwrap()) as usize;
    let mut at = u32::from_le_bytes(end[16..20].try_into().unwrap());
    let mut cd = vec![0; size];
    apk.seek(SeekFrom::Start(at.into())).map_err(read)?;
    apk.read_exact(&mut cd).map_err(read)?;
    if v1 {
        let files = {
            let mut zip = zip::ZipArchive::new(&mut *apk)?;
            // signature files there already would be duplicated: leave v1 out then
            let has_v1 = zip
                .file_names()
                .any(|n| n.is_ok_and(|n| crate::rebuild::is_v1_signature(&n)));
            if has_v1 {
                Vec::new()
            } else {
                v1::files(&mut zip, key)?
            }
        };
        if !files.is_empty() {
            // the new entries go where the central directory was; it moves behind them
            apk.seek(SeekFrom::Start(at.into())).map_err(Error::Write)?;
            let mut off = at;
            for (name, data) in &files {
                let (local, record) = v1::stored_entry(name, data, off);
                apk.write_all(&local).map_err(Error::Write)?;
                off = u32::try_from(u64::from(off) + local.len() as u64)
                    .map_err(|_| Error::TooLarge("the APK".into()))?;
                cd.extend_from_slice(&record);
            }
            at = off;
            let n = files.len() as u16;
            for k in [8, 10] {
                let c = u16::from_le_bytes(end[k..k + 2].try_into().unwrap()) + n;
                end[k..k + 2].copy_from_slice(&c.to_le_bytes());
            }
            end[12..16].copy_from_slice(&(cd.len() as u32).to_le_bytes());
            end[16..20].copy_from_slice(&at.to_le_bytes());
        }
    }
    // the signature covers the entries, the central directory, and the end record as it is
    // now, pointing at where the block goes
    let mut chunks = Chunks::default();
    let mut buf = vec![0; CHUNK];
    let mut left = at as usize;
    apk.seek(SeekFrom::Start(0)).map_err(read)?;
    while left > 0 {
        let n = left.min(CHUNK);
        apk.read_exact(&mut buf[..n]).map_err(read)?;
        chunks.update(&buf[..n]);
        left -= n;
        // a block there already would be left in front of the new one
        if left == 0 && buf[..n].ends_with(b"APK Sig Block 42") {
            return Err(Error::signature("the APK is signed already"));
        }
    }
    let block = block(chunks, &cd, &end, key)?;
    let moved = u32::try_from(u64::from(at) + block.len() as u64)
        .map_err(|_| Error::TooLarge("the APK".into()))?;
    end[16..20].copy_from_slice(&moved.to_le_bytes());
    apk.seek(SeekFrom::Start(at.into())).map_err(Error::Write)?;
    for part in [&block, &cd, &end] {
        apk.write_all(part).map_err(Error::Write)?;
    }
    apk.flush().map_err(Error::Write)?;
    Ok(u64::from(moved) + (cd.len() + end.len()) as u64)
}

/// The v2 + v3 signing block of an unsigned APK: `entries` are the chunks of its entries, `cd`
/// its central directory and `end` its end record, which says the central directory is where
/// the block goes.
fn block(mut entries: Chunks, cd: &[u8], end: &[u8], key: &Key) -> Result<Vec<u8>> {
    entries.end();
    for section in [cd, end] {
        entries.update(section);
        entries.end();
    }
    let digest = entries.digest();
    let pubkey = key
        .private
        .to_public_key()
        .to_public_key_der()
        .map_err(|e| Error::signature(format!("public key: {e}")))?
        .as_bytes()
        .to_vec();
    let signer = SigningKey::<Sha256>::new(key.private.clone());
    let digests = lp(&lp(&[&RSA_SHA256.to_le_bytes()[..], &lp(&digest)].concat()));
    let certs = lp(&lp(&key.cert));
    let sigs = |data: &[u8]| {
        lp(&lp(&[
            &RSA_SHA256.to_le_bytes()[..],
            &lp(&signer.sign(data).to_vec()),
        ]
        .concat()))
    };
    // v2: digests, certificates, attributes (stripping protection: a v3 signature exists)
    let v2_attrs = lp(&lp(&[
        STRIPPING_PROTECTION.to_le_bytes(),
        3u32.to_le_bytes(),
    ]
    .concat()));
    let v2_signed = [&digests[..], &certs, &v2_attrs].concat();
    let v2_signer = [lp(&v2_signed), sigs(&v2_signed), lp(&pubkey)].concat();
    let v2 = lp(&lp(&v2_signer));
    // v3: digests, certificates, min/max SDK, attributes
    let (min, max) = (V3_MIN_SDK.to_le_bytes(), i32::MAX.to_le_bytes());
    let v3_signed = [&digests[..], &certs, &min, &max, &lp(&[])].concat();
    let v3_signer = [
        lp(&v3_signed),
        min.to_vec(),
        max.to_vec(),
        sigs(&v3_signed),
        lp(&pubkey),
    ]
    .concat();
    let v3 = lp(&lp(&v3_signer));
    let pair = |id: u32, v: &[u8]| {
        [
            &((v.len() + 4) as u64).to_le_bytes()[..],
            &id.to_le_bytes(),
            v,
        ]
        .concat()
    };
    let mut pairs = [pair(V2_ID, &v2), pair(V3_ID, &v3)].concat();
    // pad the whole block to 4096 bytes, like apksigner
    let unpadded = 8 + pairs.len() + 8 + 16;
    if unpadded % 4096 != 0 {
        // the padding pair itself takes 12 bytes, so a smaller gap rolls over to the next 4096
        let mut gap = 4096 - unpadded % 4096;
        if gap < 12 {
            gap += 4096;
        }
        pairs.extend(pair(PADDING_ID, &vec![0; gap - 12]));
    }
    let size = (pairs.len() + 8 + 16) as u64;
    Ok([
        &size.to_le_bytes()[..],
        &pairs,
        &size.to_le_bytes(),
        b"APK Sig Block 42",
    ]
    .concat())
}

// ---------- verification ----------

fn take_lp(d: &mut &[u8]) -> Option<Vec<u8>> {
    let n = u32::from_le_bytes(d.get(..4)?.try_into().ok()?) as usize;
    let v = d.get(4..4 + n)?.to_vec();
    *d = &d[4 + n..];
    Some(v)
}

fn take_u32(d: &mut &[u8]) -> Option<u32> {
    let v = u32::from_le_bytes(d.get(..4)?.try_into().ok()?);
    *d = &d[4..];
    Some(v)
}

/// Check one v2/v3 scheme value against the file's content digest. `Ok(Some(alg))`: signed
/// with an algorithm this can't check (DSA, ECDSA, RSA-PSS, SHA-512).
fn verify_scheme(value: &[u8], v3: bool, digest: &[u8]) -> Result<Option<u32>> {
    let mut d = value;
    let signers = take_lp(&mut d).ok_or_else(|| Error::signature("bad block"))?;
    let mut s = &signers[..];
    let signer = take_lp(&mut s).ok_or_else(|| Error::signature("no signer"))?;
    let mut r = &signer[..];
    let signed = take_lp(&mut r).ok_or_else(|| Error::signature("no signed data"))?;
    if v3 {
        take_u32(&mut r);
        take_u32(&mut r);
    }
    let sigs = take_lp(&mut r).ok_or_else(|| Error::signature("no signatures"))?;
    let pubkey = take_lp(&mut r).ok_or_else(|| Error::signature("no public key"))?;
    let mut sg = &sigs[..];
    let sig = take_lp(&mut sg).ok_or_else(|| Error::signature("no signature"))?;
    let mut x = &sig[..];
    let alg = take_u32(&mut x).ok_or_else(|| Error::signature("bad signature"))?;
    if alg != RSA_SHA256 {
        return Ok(Some(alg));
    }
    let sig_bytes = take_lp(&mut x).ok_or_else(|| Error::signature("bad signature"))?;
    let key = RsaPublicKey::from_public_key_der(&pubkey)
        .map_err(|e| Error::signature(format!("public key: {e}")))?;
    let sig = Signature::try_from(&sig_bytes[..])
        .map_err(|e| Error::signature(format!("bad signature: {e}")))?;
    VerifyingKey::<Sha256>::new(key)
        .verify(&signed, &sig)
        .map_err(|_| Error::signature("signature does not match the signed data"))?;
    // the signed digest must equal the file's
    let mut sd = &signed[..];
    let digests = take_lp(&mut sd).ok_or_else(|| Error::signature("no digests"))?;
    let mut dd = &digests[..];
    let first = take_lp(&mut dd).ok_or_else(|| Error::signature("no digest"))?;
    let mut fx = &first[..];
    take_u32(&mut fx);
    let want = take_lp(&mut fx).ok_or_else(|| Error::signature("no digest"))?;
    if want != digest {
        return Err(Error::signature(
            "the APK's content doesn't match its signed digest (modified after signing)",
        ));
    }
    Ok(None)
}

/// Verify the v1 (JAR), v2 and v3 signatures of an APK file: one line per scheme.
pub fn verify(path: &str) -> Result<Vec<String>> {
    let apk = std::fs::read(path).map_err(Error::io(path))?;
    let eocd = eocd_at(&apk)?;
    let cd = u32::from_le_bytes(apk[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    let v1 = v1::verify(&mut zip::ZipArchive::new(std::io::Cursor::new(&apk))?)?;
    let block = crate::sign::signing_block(path);
    let mut out = Vec::new();
    match (&v1, &block) {
        (None, None) => return Ok(vec!["unsigned: no v1, v2 or v3 signature".to_string()]),
        (None, Some(_)) => out.push("v1: not present (Android 7+ does not need it)".to_string()),
        (Some((line, _)), _) => out.push(line.clone()),
    }
    let Some(block) = block else {
        out.push(match v1 {
            Some((_, true)) => "v2 / v3: FAILED: the v1 .SF says the APK had a v2 / v3 signature, and it was removed".to_string(),
            _ => "v2 / v3: not present (v1 only: Android 11+ refuses this for apps targeting SDK 30+)".to_string(),
        });
        return Ok(out);
    };
    let size = u64::from_le_bytes(apk[cd - 24..cd - 16].try_into().unwrap()) as usize;
    let start = cd - size - 8;
    let mut e = apk[eocd..].to_vec();
    e[16..20].copy_from_slice(&(start as u32).to_le_bytes());
    let digest = content_digest(&[&apk[..start], &apk[cd..eocd], &e]);
    for (id, name) in [(V2_ID, "v2"), (V3_ID, "v3")] {
        match block.iter().find(|b| b.0 == id) {
            Some((_, v)) => out.push(match verify_scheme(v, id == V3_ID, &digest) {
                Ok(None) => format!("{name}: verified"),
                Ok(Some(alg)) => format!("{name}: present, not checked (algorithm {alg:#06x}; only RSA with SHA-256 is checked)"),
                Err(e) => format!("{name}: FAILED: {e}"),
            }),
            None => out.push(format!("{name}: not present")),
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "../../tests/apkscope-core/apksign.rs"]
mod tests;
