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
    let (apk, cd) = crate::rebuild::rebuild(src.to_str().unwrap(), &Default::default()).unwrap();
    let signed = sign(apk, cd, &key).unwrap();
    let out = std::env::temp_dir().join(format!("apkscope-signed-{}.apk", std::process::id()));
    std::fs::write(&out, &signed).unwrap();
    assert_eq!(
        verify(out.to_str().unwrap()).unwrap(),
        vec!["v2: verified", "v3: verified"]
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
    assert!(
        verify(out.to_str().unwrap())
            .unwrap()
            .iter()
            .all(|l| l.contains("FAILED"))
    );
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}
