use super::*;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn dec<C: Decrypt>(c: &C, ct: &str) -> Vec<u8> {
    let mut b = hex(ct);
    c.decrypt(&mut b);
    b
}

/// FIPS-197 appendix C.
#[test]
fn aes_vectors() {
    let pt = hex("00112233445566778899aabbccddeeff");
    let k = |n: usize| (0..n as u8).collect::<Vec<_>>();
    assert_eq!(
        dec(
            &Aes::new(&k(16)).unwrap(),
            "69c4e0d86a7b0430d8cdb78070b4c55a"
        ),
        pt
    );
    assert_eq!(
        dec(
            &Aes::new(&k(24)).unwrap(),
            "dda97ca4864cdfe06eaf70a0ec0d7191"
        ),
        pt
    );
    assert_eq!(
        dec(
            &Aes::new(&k(32)).unwrap(),
            "8ea2b7ca516745bfeafc49904b496089"
        ),
        pt
    );
}

#[test]
fn des_vectors() {
    let d = Des::new(&hex("133457799BBCDFF1"));
    assert_eq!(dec(&d, "85E813540F0AB405"), hex("0123456789ABCDEF"));
    let mut b = hex("0123456789ABCDEF");
    d.encrypt_block(&mut b);
    assert_eq!(b, hex("85E813540F0AB405"));
    // 3DES with three equal keys is single DES
    let k3 = hex("133457799BBCDFF1133457799BBCDFF1133457799BBCDFF1");
    assert_eq!(
        dec(&Des3::new(&k3).unwrap(), "85E813540F0AB405"),
        hex("0123456789ABCDEF")
    );
}

/// RFC 2268 section 5.
#[test]
fn rc2_vectors() {
    let mut seen = [false; 256];
    pitable().iter().for_each(|&b| seen[b as usize] = true);
    assert!(seen.iter().all(|&s| s), "PITABLE is a permutation");
    assert_eq!(
        dec(&Rc2::new(&hex("0000000000000000"), 63), "ebb773f993278eff"),
        hex("0000000000000000")
    );
    assert_eq!(
        dec(&Rc2::new(&hex("ffffffffffffffff"), 64), "278b27e42e2f0d49"),
        hex("ffffffffffffffff")
    );
    assert_eq!(
        dec(&Rc2::new(&hex("3000000000000000"), 64), "30649edf9be7d2c2"),
        hex("1000000000000001")
    );
    assert_eq!(
        dec(&Rc2::new(&hex("88"), 64), "61a8a244adacccf0"),
        hex("0000000000000000")
    );
}

/// RFC 4231 test 1, RFC 6070, and the common PBKDF2-HMAC-SHA256 vector.
#[test]
fn hmac_and_pbkdf2_vectors() {
    assert_eq!(
        hmac(SHA256, &[0x0b; 20], b"Hi There"),
        hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
    );
    assert_eq!(
        pbkdf2(SHA1, b"password", b"salt", 1, 20),
        hex("0c60c80f961f0e71f3a9b524af6012062fe037a6")
    );
    assert_eq!(
        pbkdf2(SHA1, b"password", b"salt", 2, 20),
        hex("ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957")
    );
    assert_eq!(
        pbkdf2(SHA256, b"password", b"salt", 1, 32),
        hex("120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b")
    );
}

#[test]
fn cbc_padding_rejects_garbage() {
    let k = Aes::new(&[0u8; 16]).unwrap();
    assert!(cbc_decrypt(&k, &[0u8; 16], &[0x55u8; 32]).is_none());
}

#[test]
fn not_a_keystore() {
    assert!(open(b"PK\x03\x04", "x", None).is_err());
}

/// Keystores written by keytool (JKS, PKCS#12 with PBES2/AES) and `openssl pkcs12 -legacy`
/// (3DES key, RC2-40 certificates); throwaway test keys, store password `storepw1`.
#[test]
fn real_keystores() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/keystore/");
    let read = |f: &str| std::fs::read(format!("{dir}{f}")).unwrap();
    for (file, key_pass, alias) in [
        ("test.jks", Some("keypw22"), "mykey"),
        ("test.p12", None, "mykey"),
        ("legacy.p12", None, "leg"),
    ] {
        let e = pick(open(&read(file), "storepw1", key_pass).unwrap(), None).unwrap();
        assert_eq!(e.alias, alias, "{file}");
        use rsa::pkcs8::DecodePrivateKey as _;
        assert!(
            rsa::RsaPrivateKey::from_pkcs8_der(&e.key).is_ok(),
            "{file}: key"
        );
        assert!(
            crate::sign::parse_cert(&e.chain[0]).is_some(),
            "{file}: certificate"
        );
        assert!(
            open(&read(file), "wrong", key_pass).is_err(),
            "{file}: wrong password"
        );
    }
    // the JKS key has its own password
    assert!(open(&read("test.jks"), "storepw1", None).is_err());
}
