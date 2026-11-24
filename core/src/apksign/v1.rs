//! v1 (JAR) signature: `META-INF/MANIFEST.MF` with a SHA-256 digest of every entry,
//! `META-INF/CERT.SF` with a digest of the manifest and of each of its sections, and
//! `META-INF/CERT.RSA`, a PKCS#7 SignedData holding the certificate and an RSA signature of the
//! `.SF`. Android 5-6 (API < 24) verify only this; newer versions use v2/v3, and the `.SF` says
//! the APK also has those (`X-Android-APK-Signed: 2, 3`), so stripping them is detected.

use std::io::{Read, Seek};

use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::{Pkcs1v15Sign, RsaPublicKey};
use sha2::{Digest, Sha256};

use super::{Key, der_oid, seq, tlv};
use crate::{Error, Result};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64(d: &[u8]) -> String {
    let mut o = String::with_capacity(d.len().div_ceil(3) * 4);
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                o.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                o.push('=');
            }
        }
    }
    o
}

/// One manifest attribute line, wrapped as the JAR spec says: at most 72 bytes per line, a
/// continuation line starting with one space.
fn attr(name: &str, value: &str) -> String {
    let line = format!("{name}: {value}");
    let b = line.as_bytes();
    let mut o = String::new();
    let mut i = 0;
    let mut first = true;
    while i < b.len() {
        let room = if first { 72 } else { 71 };
        let mut end = (i + room).min(b.len());
        // never split a UTF-8 character
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        if !first {
            o.push(' ');
        }
        o.push_str(&line[i..end]);
        o.push_str("\r\n");
        i = end;
        first = false;
    }
    o
}

/// The three signature files for an APK whose entries are `entries` (name, uncompressed bytes
/// read through `read`), in the order given.
pub(crate) fn files<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    key: &Key,
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut manifest = String::from("Manifest-Version: 1.0\r\nCreated-By: 1.0 (apkscope)\r\n\r\n");
    let mut sf_sections = String::new();
    let mut buf = vec![0u8; 1 << 16];
    for i in 0..zip.len() {
        let mut e = zip.by_index(i)?;
        if e.is_dir() {
            continue;
        }
        let name = e
            .name()
            .map_err(|e| Error::signature(e.to_string()))?
            .into_owned();
        if crate::rebuild::is_v1_signature(&name) {
            continue;
        }
        // the digest is of the uncompressed bytes, streamed: a 100 MB library is never whole
        let mut h = Sha256::new();
        loop {
            let n = e
                .read(&mut buf)
                .map_err(|x| Error::signature(format!("{name}: {x}")))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        let section = format!(
            "{}{}\r\n",
            attr("Name", &name),
            attr("SHA-256-Digest", &base64(&h.finalize()))
        );
        sf_sections += &format!(
            "{}{}\r\n",
            attr("Name", &name),
            attr(
                "SHA-256-Digest",
                &base64(&Sha256::digest(section.as_bytes()))
            )
        );
        manifest += &section;
    }
    let sf = format!(
        "Signature-Version: 1.0\r\nCreated-By: 1.0 (apkscope)\r\n{}X-Android-APK-Signed: 2, 3\r\n\r\n{sf_sections}",
        attr(
            "SHA-256-Digest-Manifest",
            &base64(&Sha256::digest(manifest.as_bytes()))
        )
    );
    let signature = SigningKey::<Sha256>::new(key.private.clone())
        .sign(sf.as_bytes())
        .to_vec();
    let rsa = signed_data(&key.cert, &signature)?;
    Ok(vec![
        ("META-INF/MANIFEST.MF".into(), manifest.into_bytes()),
        ("META-INF/CERT.SF".into(), sf.into_bytes()),
        ("META-INF/CERT.RSA".into(), rsa),
    ])
}

/// (tag, start of content, end) of the DER element at `at`.
fn der_at(d: &[u8], at: usize) -> Option<(u8, usize, usize)> {
    let tag = *d.get(at)?;
    let first = *d.get(at + 1)? as usize;
    let (len, head) = if first < 0x80 {
        (first, 2)
    } else {
        let n = first & 0x7f;
        let mut len = 0usize;
        for k in 0..n {
            len = len << 8 | *d.get(at + 2 + k)? as usize;
        }
        (len, 2 + n)
    };
    let end = at.checked_add(head)?.checked_add(len)?;
    (end <= d.len()).then_some((tag, at + head, end))
}

/// The certificate's issuer and serial number, as DER elements.
fn issuer_and_serial(cert: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (_, cert_body, _) = der_at(cert, 0)?;
    let (_, tbs, _) = der_at(cert, cert_body)?;
    let mut at = tbs;
    // [0] version, when present
    if cert.get(at) == Some(&0xa0) {
        at = der_at(cert, at)?.2;
    }
    let serial_end = der_at(cert, at)?.2;
    let serial = cert[at..serial_end].to_vec();
    let alg_end = der_at(cert, serial_end)?.2;
    let issuer_end = der_at(cert, alg_end)?.2;
    Some((cert[alg_end..issuer_end].to_vec(), serial))
}

/// PKCS#7 SignedData, detached: SHA-256 with RSA, the certificate, one signer, no
/// authenticated attributes (what Android's JAR verifier reads).
fn signed_data(cert: &[u8], signature: &[u8]) -> Result<Vec<u8>> {
    let (issuer, serial) = issuer_and_serial(cert)
        .ok_or_else(|| Error::signature("cannot read the certificate's issuer and serial"))?;
    let sha256 = seq(&[der_oid("2.16.840.1.101.3.4.2.1"), vec![0x05, 0x00]]);
    let rsa = seq(&[der_oid("1.2.840.113549.1.1.1"), vec![0x05, 0x00]]);
    let one = tlv(0x02, &[1]);
    let signer = seq(&[
        one.clone(),
        seq(&[issuer, serial]),
        sha256.clone(),
        rsa,
        tlv(0x04, signature),
    ]);
    let signed = seq(&[
        one,
        tlv(0x31, &sha256),
        seq(&[der_oid("1.2.840.113549.1.7.1")]),
        tlv(0xa0, cert),
        tlv(0x31, &signer),
    ]);
    Ok(seq(&[der_oid("1.2.840.113549.1.7.2"), tlv(0xa0, &signed)]))
}

/// A stored entry: its local header with the data, and its central directory record.
pub(crate) fn stored_entry(name: &str, data: &[u8], offset: u32) -> (Vec<u8>, Vec<u8>) {
    let crc = crc32fast::hash(data);
    let n = name.len() as u16;
    let len = data.len() as u32;
    let mut local = Vec::with_capacity(30 + name.len() + data.len());
    local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    local.extend_from_slice(&20u16.to_le_bytes()); // version needed
    local.extend_from_slice(&0u16.to_le_bytes()); // flags
    local.extend_from_slice(&0u16.to_le_bytes()); // stored
    local.extend_from_slice(&0u16.to_le_bytes()); // time
    local.extend_from_slice(&0x221u16.to_le_bytes()); // date: 1981-01-01, as Android's tools write
    local.extend_from_slice(&crc.to_le_bytes());
    local.extend_from_slice(&len.to_le_bytes());
    local.extend_from_slice(&len.to_le_bytes());
    local.extend_from_slice(&n.to_le_bytes());
    local.extend_from_slice(&0u16.to_le_bytes()); // extra
    local.extend_from_slice(name.as_bytes());
    local.extend_from_slice(data);
    let mut cd = Vec::with_capacity(46 + name.len());
    cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    cd.extend_from_slice(&20u16.to_le_bytes()); // made by
    cd.extend_from_slice(&20u16.to_le_bytes()); // needed
    cd.extend_from_slice(&0u16.to_le_bytes());
    cd.extend_from_slice(&0u16.to_le_bytes());
    cd.extend_from_slice(&0u16.to_le_bytes());
    cd.extend_from_slice(&0x221u16.to_le_bytes());
    cd.extend_from_slice(&crc.to_le_bytes());
    cd.extend_from_slice(&len.to_le_bytes());
    cd.extend_from_slice(&len.to_le_bytes());
    cd.extend_from_slice(&n.to_le_bytes());
    cd.extend_from_slice(&[0u8; 12]); // extra, comment, disk, internal attrs, external attrs
    cd.extend_from_slice(&offset.to_le_bytes());
    cd.extend_from_slice(name.as_bytes());
    (local, cd)
}

// ---------- verification ----------

/// A digest algorithm a JAR manifest or PKCS#7 block names, weakest first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Alg {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

/// A running digest of one [`Alg`].
enum Hasher {
    Sha1(sha1::Sha1),
    Sha256(Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
}

impl Hasher {
    fn update(&mut self, d: &[u8]) {
        match self {
            Hasher::Sha1(h) => h.update(d),
            Hasher::Sha256(h) => h.update(d),
            Hasher::Sha384(h) => h.update(d),
            Hasher::Sha512(h) => h.update(d),
        }
    }

    fn finish(self) -> Vec<u8> {
        match self {
            Hasher::Sha1(h) => h.finalize().to_vec(),
            Hasher::Sha256(h) => h.finalize().to_vec(),
            Hasher::Sha384(h) => h.finalize().to_vec(),
            Hasher::Sha512(h) => h.finalize().to_vec(),
        }
    }
}

impl Alg {
    /// The algorithm an attribute name starts with: `SHA1`, `SHA-256`...
    fn named(name: &str) -> Option<Alg> {
        match name.to_ascii_uppercase().as_str() {
            "SHA1" | "SHA-1" => Some(Alg::Sha1),
            "SHA-256" => Some(Alg::Sha256),
            "SHA-384" => Some(Alg::Sha384),
            "SHA-512" => Some(Alg::Sha512),
            _ => None,
        }
    }

    /// The algorithm of a digestAlgorithm OID (its content bytes).
    fn of_oid(oid: &[u8]) -> Option<Alg> {
        const NIST: [u8; 8] = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02];
        match oid {
            [0x2b, 0x0e, 0x03, 0x02, 0x1a] => Some(Alg::Sha1),
            [nist @ .., n] if *nist == NIST => match n {
                1 => Some(Alg::Sha256),
                2 => Some(Alg::Sha384),
                3 => Some(Alg::Sha512),
                _ => None,
            },
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Alg::Sha1 => "SHA-1",
            Alg::Sha256 => "SHA-256",
            Alg::Sha384 => "SHA-384",
            Alg::Sha512 => "SHA-512",
        }
    }

    fn hasher(self) -> Hasher {
        match self {
            Alg::Sha1 => Hasher::Sha1(sha1::Sha1::new()),
            Alg::Sha256 => Hasher::Sha256(Sha256::new()),
            Alg::Sha384 => Hasher::Sha384(sha2::Sha384::new()),
            Alg::Sha512 => Hasher::Sha512(sha2::Sha512::new()),
        }
    }

    fn digest(self, d: &[u8]) -> Vec<u8> {
        let mut h = self.hasher();
        h.update(d);
        h.finish()
    }

    /// The DER DigestInfo prefix PKCS#1 v1.5 puts before a digest of this algorithm.
    fn digest_info(self) -> &'static [u8] {
        match self {
            Alg::Sha1 => &[
                0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04,
                0x14,
            ],
            Alg::Sha256 => &[
                0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x01, 0x05, 0x00, 0x04, 0x20,
            ],
            Alg::Sha384 => &[
                0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x02, 0x05, 0x00, 0x04, 0x30,
            ],
            Alg::Sha512 => &[
                0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x03, 0x05, 0x00, 0x04, 0x40,
            ],
        }
    }
}

pub(crate) fn unbase64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        acc = acc << 6 | B64.iter().position(|&x| x == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// One section of a JAR manifest or `.SF`: its bytes as stored (with the blank line that ends
/// it) and its attributes, continuation lines joined.
struct Section<'a> {
    raw: &'a [u8],
    attrs: Vec<(String, String)>,
}

impl Section<'_> {
    fn get(&self, name: &str) -> Option<&str> {
        let a = self
            .attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name));
        a.map(|(_, v)| v.as_str())
    }

    /// The strongest digest among the section's `<ALG><suffix>` attributes (`-Digest`,
    /// `-Digest-Manifest`).
    fn digest(&self, suffix: &str) -> Option<(Alg, Vec<u8>)> {
        let suffix = suffix.to_ascii_lowercase();
        let mut best: Option<(Alg, &str)> = None;
        for (k, v) in &self.attrs {
            let k = k.to_ascii_lowercase();
            let Some(alg) = k.strip_suffix(suffix.as_str()).and_then(Alg::named) else {
                continue;
            };
            if best.is_none_or(|(b, _)| alg > b) {
                best = Some((alg, v));
            }
        }
        best.and_then(|(alg, v)| Some((alg, unbase64(v)?)))
    }
}

/// The sections of a manifest or `.SF`, the main one first. Lines end with CR LF, LF or CR; a
/// line starting with a space continues the one before.
fn sections(d: &[u8]) -> Vec<Section<'_>> {
    let mut out = Vec::new();
    let (mut start, mut at) = (0, 0);
    let mut attrs: Vec<(String, String)> = Vec::new();
    while at < d.len() {
        let end = (d[at..].iter())
            .position(|&b| b == b'\n' || b == b'\r')
            .map_or(d.len(), |p| at + p);
        let line = &d[at..end];
        let mut next = end;
        if d.get(next) == Some(&b'\r') {
            next += 1;
        }
        if d.get(next) == Some(&b'\n') {
            next += 1;
        }
        if line.is_empty() {
            if !attrs.is_empty() {
                let attrs = std::mem::take(&mut attrs);
                out.push(Section {
                    raw: &d[start..next],
                    attrs,
                });
            }
            start = next;
        } else if let Some(more) = line.strip_prefix(b" ") {
            if let Some(last) = attrs.last_mut() {
                last.1.push_str(&String::from_utf8_lossy(more));
            }
        } else if let Some((k, v)) = String::from_utf8_lossy(line).split_once(':') {
            attrs.push((k.to_string(), v.strip_prefix(' ').unwrap_or(v).to_string()));
        }
        at = next;
    }
    if !attrs.is_empty() {
        out.push(Section {
            raw: &d[start..],
            attrs,
        });
    }
    out
}

/// The DER elements from `at` to `end`: (tag, start of content, end of element).
fn elements(d: &[u8], mut at: usize, end: usize) -> Vec<(u8, usize, usize)> {
    let mut v = Vec::new();
    while at < end {
        let Some((tag, start, e)) = der_at(d, at) else {
            break;
        };
        v.push((tag, start, e));
        at = e;
    }
    v
}

/// What a PKCS#7 signature block says about its `.SF`.
struct SignerInfo {
    alg: Alg,
    /// the signed attributes as the SET that was signed, when there are any
    signed_attrs: Option<Vec<u8>>,
    /// the messageDigest attribute: the digest of the `.SF`
    message_digest: Option<Vec<u8>>,
    signature: Vec<u8>,
    /// the signer's certificate
    cert: Vec<u8>,
}

/// The first signer of a PKCS#7 SignedData, with the certificate whose issuer and serial it
/// names.
fn signer(d: &[u8]) -> Option<SignerInfo> {
    let (_, ci, ci_end) = der_at(d, 0)?;
    let &(_, explicit, _) = elements(d, ci, ci_end).get(1)?;
    let (_, sd, sd_end) = der_at(d, explicit)?;
    let fields = elements(d, sd, sd_end);
    let mut certs: Vec<&[u8]> = Vec::new();
    if let Some(&(_, s, e)) = fields.iter().find(|f| f.0 == 0xa0) {
        let mut from = s;
        for (_, _, end) in elements(d, s, e) {
            certs.push(&d[from..end]);
            from = end;
        }
    }
    let &(_, set, set_end) = fields.iter().rev().find(|f| f.0 == 0x31)?;
    let &(_, si, si_end) = elements(d, set, set_end).first()?;
    // version, issuerAndSerialNumber, digestAlgorithm, [0] signed attributes (optional),
    // signatureAlgorithm, signature
    let parts = elements(d, si, si_end);
    let &(_, ias, ias_end) = parts.get(1)?;
    let &(_, alg, alg_end) = parts.get(2)?;
    let &(oid_tag, oid, oid_end) = elements(d, alg, alg_end).first()?;
    if oid_tag != 0x06 {
        return None;
    }
    let alg = Alg::of_oid(&d[oid..oid_end])?;
    let (mut signed_attrs, mut message_digest) = (None, None);
    let mut k = 3;
    if let Some(&(0xa0, s, e)) = parts.get(k) {
        // what was signed is the attributes with the SET OF tag, not the [0] one
        let mut set = d[parts[k - 1].2..e].to_vec();
        set[0] = 0x31;
        signed_attrs = Some(set);
        for (_, a, a_end) in elements(d, s, e) {
            let attr = elements(d, a, a_end);
            let (Some(&(_, o, o_end)), Some(&(_, vs, vs_end))) = (attr.first(), attr.get(1)) else {
                continue;
            };
            // 1.2.840.113549.1.9.4: messageDigest
            if d[o..o_end] == [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x04] {
                message_digest =
                    (elements(d, vs, vs_end).first()).map(|&(_, v, v_end)| d[v..v_end].to_vec());
            }
        }
        k += 1;
    }
    let &(_, sig, sig_end) = parts.get(k + 1)?;
    let names = &d[ias..ias_end];
    let cert = certs
        .into_iter()
        .find(|c| issuer_and_serial(c).is_some_and(|(i, s)| [i, s].concat() == names))?;
    Some(SignerInfo {
        alg,
        signed_attrs,
        message_digest,
        signature: d[sig..sig_end].to_vec(),
        cert: cert.to_vec(),
    })
}

/// The certificate's SubjectPublicKeyInfo (DER) and whether its key is RSA.
fn public_key(cert: &[u8]) -> Option<(&[u8], bool)> {
    let (_, body, body_end) = der_at(cert, 0)?;
    let &(_, tbs, tbs_end) = elements(cert, body, body_end).first()?;
    let mut f = elements(cert, tbs, tbs_end);
    if f.first().is_some_and(|x| x.0 == 0xa0) {
        f.remove(0);
    }
    // serial, signature algorithm, issuer, validity, subject, subjectPublicKeyInfo
    let &(_, spki, spki_end) = f.get(5)?;
    let start = f[4].2;
    let &(_, alg, alg_end) = elements(cert, spki, spki_end).first()?;
    let &(_, oid, oid_end) = elements(cert, alg, alg_end).first()?;
    // 1.2.840.113549.1.1.1: rsaEncryption
    let rsa = cert[oid..oid_end] == [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    Some((&cert[start..spki_end], rsa))
}

/// Read a whole entry.
fn entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut d = Vec::new();
    zip.by_name(name)?
        .read_to_end(&mut d)
        .map_err(|e| Error::signature(format!("{name}: {e}")))?;
    Ok(d)
}

/// Check the v1 (JAR) signature the way Android 5-6 do: each signature block signs its `.SF`,
/// the `.SF` holds the digest of the manifest (or of each of its sections), and the manifest
/// the digest of every entry, so an entry changed, added or removed after signing fails.
/// `None` when the APK has no v1 signature; else one line, and whether a `.SF` says the APK
/// also has a v2 / v3 signature (`X-Android-APK-Signed`).
pub(crate) fn verify<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<Option<(String, bool)>> {
    let names: Vec<String> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.to_string()))
        .collect();
    let sig_file = |n: &String, ext: &str| {
        let up = n.to_ascii_uppercase();
        up.starts_with("META-INF/") && !up[9..].contains('/') && up.ends_with(ext)
    };
    let mut sfs: Vec<&String> = names.iter().filter(|n| sig_file(n, ".SF")).collect();
    if sfs.is_empty() {
        return Ok(None);
    }
    sfs.sort();
    let fail = |why: String| Ok(Some((format!("v1: FAILED: {why}"), false)));
    let Some(manifest_name) = names
        .iter()
        .find(|n| n.eq_ignore_ascii_case("META-INF/MANIFEST.MF"))
        .cloned()
    else {
        return fail("no META-INF/MANIFEST.MF".into());
    };
    let manifest = entry(zip, &manifest_name)?;
    let man = sections(&manifest);
    let mut claims_v2 = false;
    let mut unchecked = Vec::new();
    let mut sf_alg = Alg::Sha1;
    for sf_name in sfs {
        let stem = &sf_name[..sf_name.len() - 3];
        let Some(block_name) = names.iter().find(|n| {
            [".RSA", ".DSA", ".EC"]
                .iter()
                .any(|x| n.eq_ignore_ascii_case(&format!("{stem}{x}")))
        }) else {
            return fail(format!(
                "{sf_name} has no signature block (.RSA / .DSA / .EC)"
            ));
        };
        let sf = entry(zip, sf_name)?;
        let block = entry(zip, block_name)?;
        let Some(s) = signer(&block) else {
            return fail(format!("cannot read the signature block {block_name}"));
        };
        sf_alg = sf_alg.max(s.alg);
        if let Some(want) = &s.message_digest
            && *want != s.alg.digest(&sf)
        {
            return fail(format!("{block_name} signed a different {sf_name}"));
        }
        let signed = s.signed_attrs.as_deref().unwrap_or(&sf);
        match public_key(&s.cert) {
            Some((spki, true)) => {
                let key = RsaPublicKey::from_public_key_der(spki)
                    .map_err(|e| Error::signature(format!("{block_name}: public key: {e}")))?;
                let scheme = Pkcs1v15Sign {
                    hash_len: Some(s.alg.digest(b"").len()),
                    prefix: s.alg.digest_info().into(),
                };
                if key
                    .verify(scheme, &s.alg.digest(signed), &s.signature)
                    .is_err()
                {
                    return fail(format!("{block_name} does not sign {sf_name}"));
                }
            }
            Some((_, false)) => unchecked.push(block_name.clone()),
            None => return fail(format!("cannot read the certificate in {block_name}")),
        }
        // the .SF covers the manifest: whole, or else section by section
        let sfx = sections(&sf);
        let Some(main) = sfx.first() else {
            return fail(format!("{sf_name} is empty"));
        };
        claims_v2 |= main
            .get("X-Android-APK-Signed")
            .is_some_and(|v| v.split(',').any(|x| matches!(x.trim(), "2" | "3")));
        let whole = main
            .digest("-Digest-Manifest")
            .is_some_and(|(alg, want)| alg.digest(&manifest) == want);
        if !whole {
            for m in man.iter().skip(1) {
                let Some(name) = m.get("Name") else { continue };
                let Some(entry) = sfx.iter().find(|x| x.get("Name") == Some(name)) else {
                    return fail(format!(
                        "{name} is in MANIFEST.MF but not signed by {sf_name}"
                    ));
                };
                match entry.digest("-Digest") {
                    Some((alg, want)) if alg.digest(m.raw) == want => {}
                    _ => {
                        return fail(format!(
                            "the MANIFEST.MF section of {name} changed after signing"
                        ));
                    }
                }
            }
        }
    }
    // every entry against its manifest digest, streamed
    let mut listed: std::collections::HashMap<&str, &Section> = std::collections::HashMap::new();
    for m in man.iter().skip(1) {
        if let Some(n) = m.get("Name") {
            listed.insert(n, m);
        }
    }
    let mut seen = 0;
    let mut buf = vec![0u8; 1 << 16];
    for i in 0..zip.len() {
        let mut e = zip.by_index(i)?;
        let name = (e.name())
            .map_err(|x| Error::signature(x.to_string()))?
            .into_owned();
        if e.is_dir() || crate::rebuild::is_v1_signature(&name) {
            continue;
        }
        // Android leaves META-INF/ out of v1 (apksigner does too): checked only when listed
        let Some(sec) = listed.get(name.as_str()) else {
            if name.starts_with("META-INF/") {
                continue;
            }
            return fail(format!(
                "{name} is not in MANIFEST.MF (added after signing)"
            ));
        };
        let Some((alg, want)) = sec.digest("-Digest") else {
            return fail(format!("{name} has no digest in MANIFEST.MF"));
        };
        let mut h = alg.hasher();
        loop {
            let n = (e.read(&mut buf)).map_err(|x| Error::signature(format!("{name}: {x}")))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        if h.finish() != want {
            return fail(format!("{name} changed after signing"));
        }
        seen += 1;
    }
    if seen < listed.len() {
        let present: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
        let gone = listed.keys().find(|n| !present.contains(*n)).copied();
        return fail(format!(
            "{} is in MANIFEST.MF but not in the APK (removed after signing)",
            gone.unwrap_or("an entry")
        ));
    }
    let line = match unchecked.is_empty() {
        true => format!("v1: verified ({seen} entries, {})", sf_alg.name()),
        false => format!(
            "v1: entry and manifest digests verified ({seen} entries); the signature in {} is not checked (only RSA is)",
            unchecked.join(", ")
        ),
    };
    Ok(Some((line, claims_v2)))
}
