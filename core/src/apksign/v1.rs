//! v1 (JAR) signature: `META-INF/MANIFEST.MF` with a SHA-256 digest of every entry,
//! `META-INF/CERT.SF` with a digest of the manifest and of each of its sections, and
//! `META-INF/CERT.RSA`, a PKCS#7 SignedData holding the certificate and an RSA signature of the
//! `.SF`. Android 5-6 (API < 24) verify only this; newer versions use v2/v3, and the `.SF` says
//! the APK also has those (`X-Android-APK-Signed: 2, 3`), so stripping them is detected.

use std::io::{Read, Seek};

use rsa::pkcs1v15::SigningKey;
use rsa::signature::{SignatureEncoding, Signer};
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
