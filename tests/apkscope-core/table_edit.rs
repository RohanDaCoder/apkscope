use super::*;

fn pool(strings: &[&str]) -> StringPool {
    StringPool {
        utf8: true,
        sorted: false,
        strings: strings.iter().map(|s| s.to_string()).collect(),
        styles: vec![],
    }
}

fn string(i: u32) -> EntryValue {
    EntryValue::Simple(Value {
        ty: Value::STRING,
        data: i,
    })
}

/// One package with one string, `hello`.
fn sample() -> Table {
    let default = config::default();
    Table {
        strings: pool(&["Hello"]),
        packages: vec![Package {
            id: 0x7f,
            name: "com.example".into(),
            last_public_type: 0,
            last_public_key: 0,
            type_id_offset: 0,
            types: pool(&["string"]),
            keys: pool(&["hello"]),
            chunks: vec![
                Chunk::Spec(Spec {
                    id: 1,
                    res0: 0,
                    res1: 1,
                    flags: vec![0],
                }),
                Chunk::Type(Type {
                    id: 1,
                    flags: 0,
                    reserved: 0,
                    config: default,
                    entries: vec![Some(Entry {
                        flags: 0,
                        key: 0,
                        value: string(0),
                    })],
                }),
            ],
        }],
    }
}

#[test]
fn sets_adds_and_removes() {
    let mut t = sample();
    let (default, night) = (config::default(), config::parse("night").unwrap());
    let hola = t.strings.intern("Hola");
    let p = t.package_mut(0x7f).unwrap();
    let hello = p.find("string", "hello").unwrap();
    assert_eq!(hello, 0x7f01_0000);
    assert_eq!(p.name(hello), Some(("string", "hello")));

    // a value in a new configuration adds its type chunk and marks what the entry varies by
    p.set(hello, &night, string(hola)).unwrap();
    assert_eq!(p.get(hello, &night).unwrap().value, string(hola));
    assert_eq!(p.spec(1).unwrap().flags, [config::bits::UI_MODE]);
    assert_eq!(p.spec(1).unwrap().res1, 2);

    // a new entry gets the next ID in its type, a new type the next type ID
    let other = p.add("string", "other", &default, string(0)).unwrap();
    assert_eq!(other, 0x7f01_0001);
    let flag = EntryValue::Simple(Value {
        ty: Value::INT_BOOLEAN,
        data: u32::MAX,
    });
    let on = p.add("bool", "on", &default, flag.clone()).unwrap();
    assert_eq!(on, 0x7f02_0000);
    assert_eq!(p.add("bool", "on", &night, flag).unwrap(), on);

    assert!(p.remove(hello, &night));
    assert!(!p.remove(hello, &night));
    assert_eq!(p.spec(1).unwrap().flags[0], 0);
    // the string type has no night values left, so no night chunk
    assert_eq!(p.types_of(1).count(), 1);
    assert_eq!(p.spec(1).unwrap().res1, 1);

    // what was written reads back the same, and decodes to the values put in
    let bytes = write(&t);
    let back = read(&bytes).unwrap();
    assert_eq!(back, t);
    let names = crate::res::Names::new(&back);
    let decoded = crate::res::decode_table(&back, &names).values;
    assert!(decoded["values/strings.xml"].contains("<string name=\"other\">Hello</string>"));
    assert!(decoded["values-night/bools.xml"].contains("<bool name=\"on\">true</bool>"));
    assert!(!decoded.contains_key("values-night/strings.xml"));
}

#[test]
fn set_needs_a_resource_that_is_there() {
    let mut t = sample();
    let p = t.package_mut(0x7f).unwrap();
    assert!(matches!(
        p.set(0x7f01_0009, &config::default(), string(0)),
        Err(Error::EntryNotFound(_))
    ));
}
