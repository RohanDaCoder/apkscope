use super::*;

#[test]
fn der_lengths_and_oids() {
    // SEQUENCE { INTEGER 5 } then a trailing byte
    let (t, c, rest) = der(&[0x30, 0x03, 0x02, 0x01, 0x05, 0xff]).unwrap();
    assert_eq!((t, c, rest), (0x30, &[0x02, 0x01, 0x05][..], &[0xff][..]));
    // long-form length
    let long = [&[0x04, 0x81, 0x80][..], &[0u8; 0x80]].concat();
    assert_eq!(der(&long).unwrap().1.len(), 0x80);
    assert_eq!(oid(&[0x55, 0x04, 0x03]), "2.5.4.3");
    assert_eq!(
        oid(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01]),
        "1.2.840.113549.1.1.1"
    );
    assert_eq!(time(0x17, b"240131120000Z"), "2024-01-31");
}

/// Reads the certificate of the local test APKs when they are present (skipped elsewhere).
#[test]
fn real_apk_certificates() {
    for apk in ["D:/APKS/Zestle_v0.6.8-prealpha.apk", "D:/APKS/Unciv.apk"] {
        if !std::path::Path::new(apk).exists() {
            continue;
        }
        let block = signing_block(apk).expect("signing block");
        let v2 = block.iter().find(|b| b.0 == V2_ID).expect("v2 scheme");
        let c = parse_cert(&scheme_cert(&v2.1).expect("v2 cert")).expect("parsed cert");
        assert!(
            !c.subject.is_empty() && c.sha256.len() == 95,
            "{apk}: {} / {}",
            c.subject,
            c.sha256
        );
        eprintln!(
            "{apk}: {} | {} .. {} | {}",
            c.subject, c.not_before, c.not_after, c.sha256
        );
    }
}

#[test]
fn length_prefixed() {
    let d = [2, 0, 0, 0, 7, 8, 1, 0, 0, 0, 9];
    assert_eq!(prefixed(&d), vec![&[7, 8][..], &[9][..]]);
}
