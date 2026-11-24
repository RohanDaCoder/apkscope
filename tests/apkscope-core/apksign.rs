use super::*;

#[test]
fn dates_and_der() {
    assert_eq!(civil(0), (1970, 1, 1));
    assert_eq!(civil(19_723), (2024, 1, 1));
    assert_eq!(der_time(0), tlv(0x17, b"700101000000Z"));
    assert_eq!(der_oid("2.5.4.3"), vec![0x06, 0x03, 0x55, 0x04, 0x03]);
    assert_eq!(&tlv(0x04, &[0; 200])[..3], &[0x04, 0x81, 200]);
}

#[test]
fn sign_then_verify_and_detect_tampering() {
    let key = generate("apkscope test").unwrap();
    let cert = crate::sign::parse_cert(&key.cert).unwrap();
    assert_eq!(cert.subject, "CN=apkscope test");
    let src = std::env::temp_dir().join(format!("apkscope-sign-{}.apk", std::process::id()));
    {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::fs::File::create(&src).unwrap());
        w.start_file("classes.dex", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(&[3u8; 5000]).unwrap();
        w.finish().unwrap();
    }
    let mut signed = std::io::Cursor::new(Vec::new());
    let edits = Default::default();
    crate::rebuild::rebuild(src.to_str().unwrap(), &edits, &mut signed).unwrap();
    sign(&mut signed, &key).unwrap();
    // a second signature would leave the first block in front of it
    assert!(sign(&mut signed, &key).is_err());
    let signed = signed.into_inner();
    let out = std::env::temp_dir().join(format!("apkscope-signed-{}.apk", std::process::id()));
    std::fs::write(&out, &signed).unwrap();
    assert_eq!(
        verify(out.to_str().unwrap()).unwrap(),
        vec![
            "v1: not present (Android 7+ does not need it)",
            "v2: verified",
            "v3: verified"
        ]
    );
    // still a valid zip
    assert!(
        zip::ZipArchive::new(std::fs::File::open(&out).unwrap())
            .unwrap()
            .by_name("classes.dex")
            .is_ok()
    );
    // flip one byte of the file content: both schemes must fail
    let mut bad = signed.clone();
    bad[40] ^= 1;
    std::fs::write(&out, &bad).unwrap();
    let lines = verify(out.to_str().unwrap()).unwrap();
    assert!(lines[1..].iter().all(|l| l.contains("FAILED")), "{lines:?}");
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn v1_signature_digests_every_entry_and_signs_the_sf() {
    use rsa::pkcs1v15::VerifyingKey;
    use rsa::signature::Verifier;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    let key = generate("apkscope v1").unwrap();
    let src = std::env::temp_dir().join(format!("apkscope-v1-{}.apk", std::process::id()));
    {
        let mut w = zip::ZipWriter::new(std::fs::File::create(&src).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("classes.dex", o).unwrap();
        w.write_all(&[7u8; 9000]).unwrap();
        // a name long enough to wrap in the manifest (72-byte lines)
        w.start_file(format!("assets/{}.json", "x".repeat(90)), o)
            .unwrap();
        w.write_all(b"{}").unwrap();
        w.finish().unwrap();
    }
    let mut signed = std::io::Cursor::new(Vec::new());
    crate::rebuild::rebuild(src.to_str().unwrap(), &Default::default(), &mut signed).unwrap();
    sign_with(&mut signed, &key, true).unwrap();
    let out = std::env::temp_dir().join(format!("apkscope-v1-signed-{}.apk", std::process::id()));
    std::fs::write(&out, signed.into_inner()).unwrap();
    // v1 verifies, and v2/v3 still do: they cover the new entries
    assert_eq!(
        verify(out.to_str().unwrap()).unwrap(),
        vec![
            "v1: verified (2 entries, SHA-256)",
            "v2: verified",
            "v3: verified"
        ]
    );
    // v1 alone catches an entry changed after signing
    let bytes = std::fs::read(&out).unwrap();
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..zin.len() {
        let mut f = zin.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut d = Vec::new();
        f.read_to_end(&mut d).unwrap();
        if name == "classes.dex" {
            d[0] ^= 1;
        }
        w.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(&d).unwrap();
    }
    let tampered = w.finish().unwrap().into_inner();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(tampered)).unwrap();
    let (line, claims_v2) = v1::verify(&mut z).unwrap().unwrap();
    assert_eq!(line, "v1: FAILED: classes.dex changed after signing");
    assert!(!claims_v2);
    let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut read = |n: &str| {
        let mut v = Vec::new();
        z.by_name(n).unwrap().read_to_end(&mut v).unwrap();
        v
    };
    let mf = String::from_utf8(read("META-INF/MANIFEST.MF")).unwrap();
    let sf = read("META-INF/CERT.SF");
    let rsa = read("META-INF/CERT.RSA");
    // every line at most 72 bytes; the long name is continued on a line starting with a space
    assert!(mf.split("\r\n").all(|l| l.len() <= 72));
    assert!(mf.contains("\r\n x"));
    // the manifest holds the digest of classes.dex
    assert!(mf.contains(&format!(
        "Name: classes.dex\r\nSHA-256-Digest: {}\r\n",
        v1::base64(&Sha256::digest([7u8; 9000]))
    )));
    // the .SF names the manifest's digest and says v2/v3 exist
    let sf_text = String::from_utf8(sf.clone()).unwrap();
    assert!(sf_text.contains(&v1::base64(&Sha256::digest(mf.as_bytes()))));
    assert!(sf_text.contains("X-Android-APK-Signed: 2, 3"));
    // CERT.RSA ends with the RSA signature of the .SF, which the certificate's key verifies
    let public = VerifyingKey::<Sha256>::new(key.private.to_public_key());
    let sig = &rsa[rsa.len() - 256..];
    assert!(public.verify(&sf, &sig.try_into().unwrap()).is_ok());
    assert!(
        rsa.windows(key.cert.len())
            .any(|w| w == key.cert.as_slice())
    );
}

#[test]
fn base64_matches_the_standard_alphabet() {
    assert_eq!(v1::base64(b""), "");
    assert_eq!(v1::base64(b"f"), "Zg==");
    assert_eq!(v1::base64(b"fo"), "Zm8=");
    assert_eq!(v1::base64(b"foo"), "Zm9v");
    assert_eq!(v1::base64(b"foobar"), "Zm9vYmFy");
}

#[test]
fn base64_decodes_what_it_encodes() {
    for d in [
        &b""[..],
        b"f",
        b"fo",
        b"foo",
        b"foobar",
        &[0xff, 0, 0x80, 7],
    ] {
        assert_eq!(v1::unbase64(&v1::base64(d)).unwrap(), d);
    }
    assert!(v1::unbase64("not*base64").is_none());
}
