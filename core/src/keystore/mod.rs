//! Signing keys from Java keystores: JKS and PKCS#12 (`.jks`, `.keystore`, `.p12`, `.pfx`),
//! the files `keytool`, Android Studio and `openssl pkcs12` write. Read only, no new
//! dependency: the ciphers these files use are in `crypto`.

mod crypto;

#[cfg(test)]
pub(crate) use crypto::*;

use crate::{Error, Result};
use crypto::{Aes, Des3, Hash, Rc2};
use sha1::Digest as _;

/// One private-key entry: its alias, the PKCS#8 key (DER) and the certificate chain (DER,
/// the signer's own certificate first).
pub struct Entry {
    pub alias: String,
    pub key: Vec<u8>,
    pub chain: Vec<Vec<u8>>,
}

fn err(m: impl Into<String>) -> Error {
    Error::Keystore(m.into())
}

/// Every private-key entry of a keystore. `key_password` defaults to the store password
/// (as keytool does).
pub fn open(data: &[u8], store_password: &str, key_password: Option<&str>) -> Result<Vec<Entry>> {
    let key_password = key_password.unwrap_or(store_password);
    match data.get(..4) {
        Some([0xfe, 0xed, 0xfe, 0xed]) => jks(data, store_password, key_password),
        Some([0xce, 0xce, 0xce, 0xce]) => Err(err(
            "JCEKS keystores are not supported: convert with `keytool -importkeystore -srcstoretype JCEKS -deststoretype PKCS12`",
        )),
        Some([0x30, ..]) => pkcs12(data, store_password, key_password),
        _ => Err(err("not a JKS or PKCS#12 keystore")),
    }
}

/// Pick the entry to sign with: by alias, or the only one.
pub fn pick(entries: Vec<Entry>, alias: Option<&str>) -> Result<Entry> {
    let names = || {
        entries
            .iter()
            .map(|e| e.alias.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match alias {
        Some(a) => {
            let list = names();
            entries
                .into_iter()
                .find(|e| e.alias.eq_ignore_ascii_case(a))
                .ok_or_else(|| err(format!("no key with alias {a} (aliases: {list})")))
        }
        None if entries.len() == 1 => Ok(entries.into_iter().next().unwrap()),
        None if entries.is_empty() => Err(err("the keystore holds no private key")),
        None => Err(err(format!(
            "the keystore holds several keys, pick one with --ks-alias ({})",
            names()
        ))),
    }
}

// ───────────────────────────── JKS ─────────────────────────────

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self
            .d
            .get(self.pos..self.pos + n)
            .ok_or_else(|| err("keystore ends early"))?;
        self.pos += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn utf(&mut self) -> Result<String> {
        let n = u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn blob(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

fn utf16be(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|c| c.to_be_bytes()).collect()
}

fn jks(data: &[u8], store_password: &str, key_password: &str) -> Result<Vec<Entry>> {
    if data.len() < 20 {
        return Err(err("keystore too short"));
    }
    let body = &data[..data.len() - 20];
    // integrity: SHA-1(password as UTF-16BE, "Mighty Aphrodite", everything before it)
    let mut h = sha1::Sha1::new();
    h.update(utf16be(store_password));
    h.update(b"Mighty Aphrodite");
    h.update(body);
    if h.finalize().as_slice() != &data[data.len() - 20..] {
        return Err(err("wrong keystore password (or a damaged keystore)"));
    }
    let mut r = Reader { d: body, pos: 4 };
    let version = r.u32()?;
    let count = r.u32()?;
    let mut out = Vec::new();
    for _ in 0..count {
        let tag = r.u32()?;
        let alias = r.utf()?;
        r.take(8)?; // date
        match tag {
            1 => {
                let protected = r.blob()?;
                let n = r.u32()?;
                let mut chain = Vec::new();
                for _ in 0..n {
                    if version == 2 {
                        r.utf()?; // certificate type
                    }
                    chain.push(r.blob()?.to_vec());
                }
                let key = jks_key(protected, key_password)?;
                out.push(Entry { alias, key, chain });
            }
            2 => {
                if version == 2 {
                    r.utf()?;
                }
                r.blob()?;
            }
            t => return Err(err(format!("unknown keystore entry type {t}"))),
        }
    }
    Ok(out)
}

/// Sun's key protection: EncryptedPrivateKeyInfo whose data is salt(20) | key XOR stream |
/// SHA-1 check(20), the stream built from SHA-1(password | previous).
fn jks_key(protected: &[u8], password: &str) -> Result<Vec<u8>> {
    let mut d = Der::new(protected);
    let mut info = d.seq()?;
    info.any()?; // algorithm
    let enc = info.octets()?;
    if enc.len() < 40 {
        return Err(err("protected key too short"));
    }
    let pw = utf16be(password);
    let (salt, body, check) = (&enc[..20], &enc[20..enc.len() - 20], &enc[enc.len() - 20..]);
    let mut key = Vec::with_capacity(body.len());
    let mut digest = salt.to_vec();
    while key.len() < body.len() {
        let mut h = sha1::Sha1::new();
        h.update(&pw);
        h.update(&digest);
        digest = h.finalize().to_vec();
        for &k in &digest {
            if key.len() == body.len() {
                break;
            }
            key.push(body[key.len()] ^ k);
        }
    }
    let mut h = sha1::Sha1::new();
    h.update(&pw);
    h.update(&key);
    if h.finalize().as_slice() != check {
        return Err(err("wrong key password (--ks-key-pass)"));
    }
    Ok(key)
}

// ───────────────────────────── DER ─────────────────────────────

/// A DER reader over one level of a structure.
struct Der<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Der<'a> {
    fn new(d: &'a [u8]) -> Der<'a> {
        Der { d, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.d.len()
    }

    fn peek(&self) -> Option<u8> {
        self.d.get(self.pos).copied()
    }

    /// (tag, contents)
    fn tlv(&mut self) -> Result<(u8, &'a [u8])> {
        let short = || err("malformed keystore (DER)");
        let tag = *self.d.get(self.pos).ok_or_else(short)?;
        let first = *self.d.get(self.pos + 1).ok_or_else(short)? as usize;
        let mut p = self.pos + 2;
        let len = if first < 0x80 {
            first
        } else {
            let n = first & 0x7f;
            if n == 0 || n > 4 {
                return Err(short());
            }
            let v = self
                .d
                .get(p..p + n)
                .ok_or_else(short)?
                .iter()
                .fold(0usize, |a, &b| a << 8 | b as usize);
            p += n;
            v
        };
        let body = self.d.get(p..p + len).ok_or_else(short)?;
        self.pos = p + len;
        Ok((tag, body))
    }

    fn expect(&mut self, tag: u8) -> Result<&'a [u8]> {
        let (t, b) = self.tlv()?;
        if t != tag {
            return Err(err(format!(
                "malformed keystore: tag {t:#x}, expected {tag:#x}"
            )));
        }
        Ok(b)
    }

    fn any(&mut self) -> Result<&'a [u8]> {
        Ok(self.tlv()?.1)
    }

    fn seq(&mut self) -> Result<Der<'a>> {
        self.expect(0x30).map(Der::new)
    }

    fn oid(&mut self) -> Result<String> {
        let b = self.expect(0x06)?;
        let mut parts = Vec::new();
        let mut v = 0u64;
        for (i, &x) in b.iter().enumerate() {
            v = v << 7 | (x & 0x7f) as u64;
            if x & 0x80 == 0 {
                if i == 0 || parts.is_empty() {
                    parts.push((v / 40).min(2));
                    parts.push(v - 40 * (v / 40).min(2));
                } else {
                    parts.push(v);
                }
                v = 0;
            }
        }
        Ok(parts
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("."))
    }

    fn octets(&mut self) -> Result<Vec<u8>> {
        let (t, b) = self.tlv()?;
        match t {
            0x04 | 0x80 => Ok(b.to_vec()),
            // constructed (BER-style) octet string: its chunks concatenated
            0x24 | 0xa0 => {
                let mut inner = Der::new(b);
                let mut out = Vec::new();
                while !inner.done() {
                    out.extend(inner.octets()?);
                }
                Ok(out)
            }
            _ => Err(err(format!(
                "malformed keystore: tag {t:#x}, expected an octet string"
            ))),
        }
    }

    fn int(&mut self) -> Result<u64> {
        Ok(self
            .expect(0x02)?
            .iter()
            .fold(0u64, |a, &b| a << 8 | b as u64))
    }
}

// ───────────────────────────── PKCS#12 ─────────────────────────────

const DATA: &str = "1.2.840.113549.1.7.1";
const ENCRYPTED_DATA: &str = "1.2.840.113549.1.7.6";
const KEY_BAG: &str = "1.2.840.113549.1.12.10.1.1";
const SHROUDED_KEY_BAG: &str = "1.2.840.113549.1.12.10.1.2";
const CERT_BAG: &str = "1.2.840.113549.1.12.10.1.3";
const X509: &str = "1.2.840.113549.1.9.22.1";
const FRIENDLY_NAME: &str = "1.2.840.113549.1.9.20";
const LOCAL_KEY_ID: &str = "1.2.840.113549.1.9.21";

/// BMPString password for the PKCS#12 KDF: UTF-16BE plus a two-byte terminator.
fn bmp(password: &str) -> Vec<u8> {
    let mut v = utf16be(password);
    v.extend([0, 0]);
    v
}

fn hash_by_oid(oid: &str) -> Option<Hash> {
    Some(match oid {
        "1.3.14.3.2.26" | "1.2.840.113549.2.7" => crypto::SHA1,
        "2.16.840.1.101.3.4.2.1" | "1.2.840.113549.2.9" => crypto::SHA256,
        "2.16.840.1.101.3.4.2.2" | "1.2.840.113549.2.10" => crypto::SHA384,
        "2.16.840.1.101.3.4.2.3" | "1.2.840.113549.2.11" => crypto::SHA512,
        _ => return None,
    })
}

/// Decrypt data protected by `alg` (an AlgorithmIdentifier's contents) with `password`.
fn pbe_decrypt(alg: &[u8], password: &str, data: &[u8]) -> Result<Vec<u8>> {
    let mut a = Der::new(alg);
    let oid = a.oid()?;
    let wrong = || err("wrong password (the keystore's data does not decrypt)");
    match oid.as_str() {
        // pbeWithSHAAnd3-KeyTripleDES-CBC, pbeWithSHAAnd40BitRC2-CBC, -128BitRC2-CBC
        "1.2.840.113549.1.12.1.3" | "1.2.840.113549.1.12.1.6" | "1.2.840.113549.1.12.1.5" => {
            let mut p = a.seq()?;
            let salt = p.expect(0x04)?;
            let iter = p.int()? as u32;
            let pw = bmp(password);
            let iv = crypto::pkcs12_kdf(crypto::SHA1, &pw, salt, 2, iter, 8);
            let out = match oid.as_str() {
                "1.2.840.113549.1.12.1.3" => {
                    let key = crypto::pkcs12_kdf(crypto::SHA1, &pw, salt, 1, iter, 24);
                    crypto::cbc_decrypt(&Des3::new(&key).unwrap(), &iv, data)
                }
                o => {
                    let n = if o.ends_with(".6") { 5 } else { 16 };
                    let key = crypto::pkcs12_kdf(crypto::SHA1, &pw, salt, 1, iter, n);
                    crypto::cbc_decrypt(&Rc2::new(&key, n * 8), &iv, data)
                }
            };
            out.ok_or_else(wrong)
        }
        // PBES2: PBKDF2 + AES / 3DES
        "1.2.840.113549.1.5.13" => {
            let mut p = a.seq()?;
            let mut kdf = p.seq()?;
            if kdf.oid()? != "1.2.840.113549.1.5.12" {
                return Err(err("PBES2 with a key derivation other than PBKDF2"));
            }
            let mut kp = kdf.seq()?;
            let salt = kp.expect(0x04)?.to_vec();
            let iter = kp.int()? as u32;
            let mut prf = crypto::SHA1;
            let mut key_len = None;
            while !kp.done() {
                match kp.peek() {
                    Some(0x02) => key_len = Some(kp.int()? as usize),
                    Some(0x30) => {
                        let mut alg = kp.seq()?;
                        let o = alg.oid()?;
                        prf = hash_by_oid(&o).ok_or_else(|| err(format!("PBKDF2 with PRF {o}")))?;
                    }
                    _ => {
                        kp.any()?;
                    }
                }
            }
            let mut enc = p.seq()?;
            let cipher = enc.oid()?;
            let iv = enc.expect(0x04)?.to_vec();
            let pw = password.as_bytes();
            let out = match cipher.as_str() {
                "2.16.840.1.101.3.4.1.2"
                | "2.16.840.1.101.3.4.1.22"
                | "2.16.840.1.101.3.4.1.42" => {
                    let n = match cipher.as_str() {
                        "2.16.840.1.101.3.4.1.2" => 16,
                        "2.16.840.1.101.3.4.1.22" => 24,
                        _ => 32,
                    };
                    let key = crypto::pbkdf2(prf, pw, &salt, iter, key_len.unwrap_or(n));
                    Aes::new(&key).and_then(|c| crypto::cbc_decrypt(&c, &iv, data))
                }
                "1.2.840.113549.3.7" => {
                    let key = crypto::pbkdf2(prf, pw, &salt, iter, 24);
                    crypto::cbc_decrypt(&Des3::new(&key).unwrap(), &iv, data)
                }
                c => return Err(err(format!("PBES2 cipher {c} is not supported"))),
            };
            out.ok_or_else(wrong)
        }
        o => Err(err(format!("keystore encryption {o} is not supported"))),
    }
}

struct Bag {
    alias: Option<String>,
    id: Option<Vec<u8>>,
}

fn bag_attrs(d: &mut Der) -> Result<Bag> {
    let mut bag = Bag {
        alias: None,
        id: None,
    };
    if d.peek() != Some(0x31) {
        return Ok(bag);
    }
    let mut set = Der::new(d.expect(0x31)?);
    while !set.done() {
        let mut attr = set.seq()?;
        let oid = attr.oid()?;
        let mut vals = Der::new(attr.expect(0x31)?);
        match oid.as_str() {
            FRIENDLY_NAME => {
                let b = vals.expect(0x1e)?;
                let u: Vec<u16> = b
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                    .collect();
                bag.alias = Some(String::from_utf16_lossy(&u));
            }
            LOCAL_KEY_ID => bag.id = Some(vals.expect(0x04)?.to_vec()),
            _ => {}
        }
    }
    Ok(bag)
}

fn pkcs12(data: &[u8], store_password: &str, key_password: &str) -> Result<Vec<Entry>> {
    let mut top = Der::new(data);
    let mut pfx = top.seq()?;
    if pfx.int()? != 3 {
        return Err(err("not a PKCS#12 keystore (version)"));
    }
    let mut auth = pfx.seq()?;
    if auth.oid()? != DATA {
        return Err(err("PKCS#12 with public-key integrity is not supported"));
    }
    let auth_bytes = Der::new(auth.expect(0xa0)?).octets()?;
    // MAC: tells a wrong store password apart from a damaged file
    if !pfx.done() {
        let mut mac = pfx.seq()?;
        let mut digest_info = mac.seq()?;
        let mut alg = digest_info.seq()?;
        let h = hash_by_oid(&alg.oid()?).ok_or_else(|| err("unsupported PKCS#12 MAC"))?;
        let want = digest_info.expect(0x04)?;
        let salt = mac.expect(0x04)?;
        let iter = if mac.done() { 1 } else { mac.int()? as u32 };
        let key = crypto::pkcs12_kdf(h, &bmp(store_password), salt, 3, iter, h.out);
        if crypto::hmac(h, &key, &auth_bytes) != want {
            return Err(err("wrong keystore password (or a damaged keystore)"));
        }
    }
    let mut keys: Vec<(Bag, Vec<u8>)> = Vec::new();
    let mut certs: Vec<(Bag, Vec<u8>)> = Vec::new();
    let mut infos = Der::new(&auth_bytes).seq()?;
    while !infos.done() {
        let mut ci = infos.seq()?;
        let kind = ci.oid()?;
        let safe = match kind.as_str() {
            DATA => Der::new(ci.expect(0xa0)?).octets()?,
            ENCRYPTED_DATA => {
                let mut ed = Der::new(ci.expect(0xa0)?).seq()?;
                ed.int()?;
                let mut eci = ed.seq()?;
                eci.oid()?;
                let alg = eci.expect(0x30)?;
                let enc = eci.octets()?;
                pbe_decrypt(alg, store_password, &enc)?
            }
            _ => continue,
        };
        let mut bags = Der::new(&safe).seq()?;
        while !bags.done() {
            let mut bag = bags.seq()?;
            let id = bag.oid()?;
            let value = bag.expect(0xa0)?;
            let attrs = bag_attrs(&mut bag)?;
            match id.as_str() {
                KEY_BAG => {
                    keys.push((attrs, Der::new(value).expect(0x30).map(|_| value.to_vec())?))
                }
                SHROUDED_KEY_BAG => {
                    let mut epki = Der::new(value).seq()?;
                    let alg = epki.expect(0x30)?;
                    let enc = epki.octets()?;
                    let key = pbe_decrypt(alg, key_password, &enc)
                        .map_err(|_| err("wrong key password (--ks-key-pass)"))?;
                    keys.push((attrs, key));
                }
                CERT_BAG => {
                    let mut cb = Der::new(value).seq()?;
                    if cb.oid()? == X509 {
                        let cert = Der::new(cb.expect(0xa0)?).octets()?;
                        certs.push((attrs, cert));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(keys
        .into_iter()
        .enumerate()
        .map(|(n, (bag, key))| {
            // the key's certificate shares its localKeyId (or its alias); else the only one
            let mine: Vec<Vec<u8>> = certs
                .iter()
                .filter(|(c, _)| {
                    (bag.id.is_some() && c.id == bag.id)
                        || (bag.alias.is_some() && c.alias == bag.alias)
                })
                .map(|(_, d)| d.clone())
                .collect();
            let chain = if mine.is_empty() && certs.len() == 1 {
                vec![certs[0].1.clone()]
            } else {
                mine
            };
            Entry {
                alias: bag.alias.unwrap_or_else(|| (n + 1).to_string()),
                key,
                chain,
            }
        })
        .collect())
}

#[cfg(test)]
#[path = "../../../tests/apkscope-core/keystore.rs"]
mod tests;
