use super::*;

#[test]
fn values_format() {
    let names = Names {
        map: HashMap::new(),
        app: 0x7f,
    };
    assert_eq!(
        format_value(Value { ty: 0x12, data: 1 }, &[], &names),
        "true"
    );
    assert_eq!(
        format_value(
            Value {
                ty: 0x1c,
                data: 0xff112233
            },
            &[],
            &names
        ),
        "#ff112233"
    );
    // 16dp: mantissa 16, radix 0, unit dp
    assert_eq!(
        format_value(
            Value {
                ty: 0x05,
                data: (16 << 8) | 1
            },
            &[],
            &names
        ),
        "16dp"
    );
    // 1.5sp: mantissa 1.5 * 2^7 = 192, radix 1
    assert_eq!(
        format_value(
            Value {
                ty: 0x05,
                data: (192 << 8) | (1 << 4) | 2
            },
            &[],
            &names
        ),
        "1.5sp"
    );
    let body = |s: &str| string_body(s, &[], &[]);
    assert_eq!(body("it's @home"), "it\\'s @home");
    assert_eq!(body("@x"), "\\@x");
    assert_eq!(body("say \"hi\"\n"), "say \\\"hi\\\"\\n");
    assert_eq!(body(" two  spaces"), "\" two  spaces\"");
}

#[test]
fn framework_names() {
    let names = Names {
        map: HashMap::new(),
        app: 0x7f,
    };
    assert_eq!(
        names.reference(0x01010054, '?'),
        "?android:attr/windowBackground"
    );
    assert_eq!(
        names.reference(0x0106000d, '@'),
        "@android:color/transparent"
    );
    assert_eq!(names.name(0x01010000), Some("theme"));
}

#[test]
fn framework_constants() {
    assert_eq!(gravity(0x11), "center");
    assert_eq!(gravity(0x00800003 | 0x10), "start|center_vertical");
    assert_eq!(
        framework_constant(
            "layout_width",
            Value {
                ty: 0x10,
                data: 0xfffffffe
            }
        )
        .as_deref(),
        Some("wrap_content")
    );
}

#[test]
fn utf8_pool() {
    // pool header (28 bytes) + 1 offset + "hi"
    let mut d = vec![0u8; 28];
    d[0..2].copy_from_slice(&1u16.to_le_bytes());
    d[2..4].copy_from_slice(&28u16.to_le_bytes());
    d[4..8].copy_from_slice(&40u32.to_le_bytes());
    d[8..12].copy_from_slice(&1u32.to_le_bytes());
    d[16..20].copy_from_slice(&0x100u32.to_le_bytes());
    d[20..24].copy_from_slice(&32u32.to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    d.extend_from_slice(&[2, 2, b'h', b'i', 0, 0, 0, 0]);
    assert_eq!(crate::table::read::pool(&d, 0).unwrap().strings, ["hi"]);
}
