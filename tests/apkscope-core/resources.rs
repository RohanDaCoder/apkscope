use super::*;
use crate::table::{
    Chunk, Entry, EntryValue, Package, Spec, StringPool, Table, Type, Value, config,
};
use std::io::{Cursor, Write};

fn pool(strings: &[&str]) -> StringPool {
    StringPool {
        utf8: true,
        sorted: false,
        strings: strings.iter().map(|s| s.to_string()).collect(),
        styles: vec![],
    }
}

/// An APK with a manifest and one string, `hello`, in the default and night configurations.
fn apk() -> Vec<u8> {
    let string = |i| {
        Some(Entry {
            flags: 0,
            key: 0,
            value: EntryValue::Simple(Value {
                ty: Value::STRING,
                data: i,
            }),
        })
    };
    let ty = |config, i| {
        Chunk::Type(Type {
            id: 1,
            flags: 0,
            reserved: 0,
            config,
            entries: vec![string(i)],
        })
    };
    let table = Table {
        strings: pool(&["Hello", "Good night"]),
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
                    res1: 2,
                    flags: vec![config::bits::UI_MODE],
                }),
                ty(config::default(), 0),
                ty(config::parse("night").unwrap(), 1),
            ],
        }],
    };
    let names = Names::new(&table);
    let manifest = crate::axml::encode(
        r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.example" android:label="@string/hello" />"#,
        None,
        &names,
    )
    .unwrap();
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    z.start_file("AndroidManifest.xml", stored).unwrap();
    z.write_all(&manifest).unwrap();
    z.start_file("resources.arsc", stored).unwrap();
    z.write_all(&table::write(&table)).unwrap();
    z.finish().unwrap().into_inner()
}

#[test]
fn decodes_and_edits_a_tree() {
    let bytes = apk();
    let mut a = Apk::new(Cursor::new(&bytes)).unwrap();
    let files = decode(&mut a).unwrap();
    let text = |p: &str| String::from_utf8(files[p].clone()).unwrap();
    assert!(text("AndroidManifest.xml").contains(r#"android:label="@string/hello""#));
    assert!(text("res/values/strings.xml").contains(r#"<string name="hello">Hello</string>"#));
    assert!(text("res/values-night/strings.xml").contains("Good night"));
    assert!(text("res/values/public.xml").contains(r#"id="0x7f010000""#));

    // what decode gives changes nothing
    assert!(edits(&mut a, &files, true).unwrap().is_empty());

    // an edited string, and a configuration that is gone
    let mut edited = files.clone();
    edited.insert(
        "res/values/strings.xml".into(),
        text("res/values/strings.xml")
            .replace(">Hello<", ">Hi<")
            .into_bytes(),
    );
    edited.remove("res/values-night/strings.xml");
    let e = edits(&mut a, &edited, true).unwrap();
    let t = table::read(e["resources.arsc"].as_ref().unwrap()).unwrap();
    let p = t.app().unwrap();
    let Some(EntryValue::Simple(v)) = p.get(0x7f01_0000, &config::default()).map(|e| &e.value)
    else {
        panic!("a string")
    };
    assert_eq!(t.strings.strings[v.data as usize], "Hi");
    // the night configuration has no values left, and so no chunk
    assert_eq!(p.values(0x7f01_0000).count(), 1);
    assert_eq!(
        p.chunks
            .iter()
            .filter(|c| matches!(c, Chunk::Type(_)))
            .count(),
        1
    );
    // without `complete`, a file that isn't given is left as it is
    let e = edits(&mut a, &edited, false).unwrap();
    let t = table::read(e["resources.arsc"].as_ref().unwrap()).unwrap();
    assert_eq!(t.app().unwrap().values(0x7f01_0000).count(), 2);

    // new files: an image, and XML that names a string added with it
    let mut added = files.clone();
    added.insert("res/drawable-xxhdpi/icon.png".into(), b"png".to_vec());
    added.insert(
        "res/xml/prefs.xml".into(),
        br#"<PreferenceScreen xmlns:android="http://schemas.android.com/apk/res/android" android:title="@string/added" />"#.to_vec(),
    );
    added.insert(
        "res/values/strings.xml".into(),
        text("res/values/strings.xml")
            .replace(
                "</resources>",
                r#"<string name="added">Added</string></resources>"#,
            )
            .into_bytes(),
    );
    let e = edits(&mut a, &added, true).unwrap();
    // the image's directory is named as aapt2 compiles it, with the version density needs
    assert_eq!(
        e["res/drawable-xxhdpi-v4/icon.png"].as_deref(),
        Some(&b"png"[..])
    );
    let t = table::read(e["resources.arsc"].as_ref().unwrap()).unwrap();
    let names = Names::new(&t);
    let p = t.app().unwrap();
    let icon = p.find("drawable", "icon").unwrap();
    let xxhdpi = config::parse("xxhdpi-v4").unwrap();
    let Some(EntryValue::Simple(v)) = p.get(icon, &xxhdpi).map(|e| &e.value) else {
        panic!("a file")
    };
    assert_eq!(
        t.strings.strings[v.data as usize],
        "res/drawable-xxhdpi-v4/icon.png"
    );
    let prefs = res::decode_xml(e["res/xml/prefs.xml"].as_ref().unwrap(), Some(&names)).unwrap();
    assert!(
        prefs.contains(r#"android:title="@string/added""#),
        "{prefs}"
    );

    // a decoded tree of that APK, with the image gone, removes it
    let rebuilt = rebuild::rebuild_from(Cursor::new(&bytes), &e).unwrap().0;
    let mut b = Apk::new(Cursor::new(&rebuilt)).unwrap();
    let mut tree = decode(&mut b).unwrap();
    assert!(tree.contains_key("res/drawable-xxhdpi-v4/icon.png"));
    tree.remove("res/drawable-xxhdpi-v4/icon.png");
    let e = edits(&mut b, &tree, true).unwrap();
    assert_eq!(e["res/drawable-xxhdpi-v4/icon.png"], None);
    let t = table::read(e["resources.arsc"].as_ref().unwrap()).unwrap();
    let p = t.app().unwrap();
    // its only value, and with it the type's only configuration, is gone
    assert!(
        p.find("drawable", "icon")
            .is_none_or(|id| p.get(id, &xxhdpi).is_none())
    );

    // anything else is not a resource
    edited.insert("assets/x".into(), vec![0]);
    assert!(matches!(
        edits(&mut a, &edited, true),
        Err(Error::EntryNotFound(_))
    ));
}
