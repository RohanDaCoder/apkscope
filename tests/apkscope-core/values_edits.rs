use super::*;
use crate::table::{Chunk, Entry, Spec, Type, config};

fn pool(strings: &[&str]) -> StringPool {
    StringPool {
        utf8: true,
        sorted: false,
        strings: strings.iter().map(|s| s.to_string()).collect(),
        styles: vec![],
    }
}

/// One package with one string, `hello`.
fn sample() -> Table {
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
                    config: config::default(),
                    entries: vec![Some(Entry {
                        flags: 0,
                        key: 0,
                        value: EntryValue::Simple(Value {
                            ty: Value::STRING,
                            data: 0,
                        }),
                    })],
                }),
            ],
        }],
    }
}

fn files(f: &[(&str, &str)]) -> BTreeMap<String, String> {
    f.iter()
        .map(|(p, t)| (p.to_string(), t.to_string()))
        .collect()
}

#[test]
fn edits_through_values_files() {
    let mut t = sample();
    let strings = r#"<resources>
    <string name="hello">Tap <b>here</b>, now</string>
    <string name="quoted">"  two  spaces " and \"quotes\"</string>
    <string name="pinned">@string/hello</string>
</resources>"#;
    apply(
        &mut t,
        &files(&[
            ("values/strings.xml", strings),
            (
                "values-night/bools.xml",
                r#"<resources><bool name="dark">true</bool></resources>"#,
            ),
            (
                "values/integers.xml",
                r#"<resources><integer name="answer">42</integer></resources>"#,
            ),
            (
                "values/public.xml",
                r#"<resources><public type="string" name="pinned" id="0x7f010005" /></resources>"#,
            ),
        ]),
        0,
    )
    .unwrap();
    let p = t.app().unwrap();
    let hello = p.find("string", "hello").unwrap();
    let EntryValue::Simple(v) = &p.get(hello, &config::default()).unwrap().value else {
        panic!("a simple value")
    };
    assert_eq!(t.strings.strings[v.data as usize], "Tap here, now");
    let spans = &t.strings.styles[v.data as usize];
    assert_eq!((spans[0].first, spans[0].last), (4, 7));
    assert_eq!(t.strings.strings[spans[0].name as usize], "b");
    assert_eq!(p.find("string", "pinned"), Some(0x7f01_0005));
    assert_eq!(p.find("string", "quoted"), Some(0x7f01_0001));
    // a new configuration gets the SDK version aapt2 gives it below the app's minimum
    let night = config::parse("night-v8").unwrap();
    assert!(p.get(p.find("bool", "dark").unwrap(), &night).is_some());
    assert_eq!(
        p.config("night", 21).unwrap(),
        config::parse("night").unwrap()
    );

    // what it decodes to says the same
    let decoded = res::decode_table(&t, &Names::new(&t)).values;
    let s = &decoded["values/strings.xml"];
    assert!(
        s.contains(r#"<string name="hello">Tap <b>here</b>, now</string>"#),
        "{s}"
    );
    assert!(
        s.contains(r#"<string name="quoted">"  two  spaces  and \"quotes\""</string>"#),
        "{s}"
    );
    assert!(
        s.contains(r#"<string name="pinned">@string/hello</string>"#),
        "{s}"
    );
    assert!(
        decoded["values/public.xml"]
            .contains(r#"<public type="string" name="pinned" id="0x7f010005" />"#)
    );
    assert!(decoded["values/integers.xml"].contains(r#"<integer name="answer">42</integer>"#));

    // a string the file no longer has is removed; its ID stays taken
    let fewer = strings.replace(
        r#"    <string name="quoted">"  two  spaces " and \"quotes\"</string>"#,
        "",
    );
    apply(&mut t, &files(&[("values/strings.xml", &fewer)]), 0).unwrap();
    let p = t.app().unwrap();
    assert_eq!(p.find("string", "quoted"), None);
    assert!(p.find("string", "hello").is_some());
    let back = crate::table::read(&crate::table::write(&t)).unwrap();
    assert_eq!(back, t);
}

#[test]
fn sets_one_resource() {
    // a value that is there is set as a values file with its element would set it
    let mut t = sample();
    assert!(
        set(
            &mut t,
            "string",
            "hello",
            "",
            Body::Text("Tap  \"here\""),
            21
        )
        .unwrap()
    );
    let mut applied = sample();
    let strings = r#"<resources><string name="hello">Tap  "here"</string></resources>"#;
    apply(&mut applied, &files(&[("values/strings.xml", strings)]), 21).unwrap();
    assert_eq!(t, applied);

    // one that is not there is added: in a new configuration, and an array by its items
    assert!(!set(&mut t, "bool", "dark", "night", Body::Text("true"), 21).unwrap());
    let items = ["1".to_owned(), "2".to_owned()];
    assert!(
        !set(
            &mut t,
            "integer-array",
            "modes",
            "",
            Body::Items(&items),
            21
        )
        .unwrap()
    );
    let decoded = res::decode_table(&t, &Names::new(&t)).values;
    assert!(decoded["values-night/bools.xml"].contains(r#"<bool name="dark">true</bool>"#));
    let arrays = &decoded["values/arrays.xml"];
    assert!(
        arrays.contains("<integer-array name=\"modes\">\n        <item>1</item>"),
        "{arrays}"
    );
    // and nothing else changed
    assert!(decoded["values/strings.xml"].contains(r#"<string name="hello">Tap here</string>"#));
}
