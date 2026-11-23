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
    // an open APK keeps its edits, so each set of them is made to its own
    let open = || Apk::new(Cursor::new(&bytes)).unwrap();
    let files = decode(&mut open()).unwrap();
    let text = |p: &str| String::from_utf8(files[p].clone()).unwrap();
    assert!(text("AndroidManifest.xml").contains(r#"android:label="@string/hello""#));
    assert!(text("res/values/strings.xml").contains(r#"<string name="hello">Hello</string>"#));
    assert!(text("res/values-night/strings.xml").contains("Good night"));
    assert!(text("res/values/public.xml").contains(r#"id="0x7f010000""#));

    // what decode gives changes nothing
    assert!(edits(&mut open(), &files, true).unwrap().is_empty());

    // an edited string, and a configuration that is gone
    let mut edited = files.clone();
    edited.insert(
        "res/values/strings.xml".into(),
        text("res/values/strings.xml")
            .replace(">Hello<", ">Hi<")
            .into_bytes(),
    );
    edited.remove("res/values-night/strings.xml");
    let e = edits(&mut open(), &edited, true).unwrap();
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
    let e = edits(&mut open(), &edited, false).unwrap();
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
    let e = edits(&mut open(), &added, true).unwrap();
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

    // anything else is a file put as it is
    edited.insert("assets/x".into(), vec![0]);
    let e = edits(&mut open(), &edited, true).unwrap();
    assert_eq!(e["assets/x"].as_deref(), Some(&[0][..]));
}

#[test]
fn an_open_apk_reads_and_changes_one_file_at_a_time() {
    let bytes = apk();
    let mut a = Apk::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(
        a.files().unwrap(),
        [
            "AndroidManifest.xml",
            "res/values-night/strings.xml",
            "res/values/public.xml",
            "res/values/strings.xml",
        ]
    );
    let text = |a: &mut Apk<_>, p: &str| String::from_utf8(a.file(p).unwrap().unwrap()).unwrap();

    // a value set in the table, where it had one and where it had none
    assert!(
        a.set_value("string", "hello", "", values::Body::Text("Hi"))
            .unwrap()
    );
    assert!(
        !a.set_value("string", "hello", "land", values::Body::Text("Wide"))
            .unwrap()
    );
    let items = ["1".to_owned(), "2".to_owned()];
    assert!(
        !a.set_value("integer-array", "modes", "", values::Body::Items(&items))
            .unwrap()
    );
    // the files say what the table has now
    assert!(text(&mut a, "res/values/strings.xml").contains(r#"<string name="hello">Hi</string>"#));
    assert!(text(&mut a, "res/values-land/strings.xml").contains("Wide"));
    assert!(text(&mut a, "res/values/arrays.xml").contains("<integer-array name=\"modes\">"));
    assert!(
        a.files()
            .unwrap()
            .contains(&"res/values/arrays.xml".to_owned())
    );

    // a file put is read back as it was put, and listed; one removed is gone
    a.put("res/raw/note.txt", b"note".to_vec()).unwrap();
    a.put("assets/x", vec![0]).unwrap();
    a.remove("res/values-night/strings.xml").unwrap();
    assert_eq!(
        a.file("res/raw/note.txt").unwrap().as_deref(),
        Some(&b"note"[..])
    );
    let files = a.files().unwrap();
    assert!(
        files.contains(&"res/raw/note.txt".to_owned()) && files.contains(&"assets/x".to_owned())
    );
    assert!(!files.contains(&"res/values-night/strings.xml".to_owned()));
    // the manifest's text, in place of its binary XML, is compiled when the APK is written
    let manifest = text(&mut a, "AndroidManifest.xml").replace("com.example", "com.other");
    a.put("AndroidManifest.xml", manifest.clone().into_bytes())
        .unwrap();
    assert_eq!(text(&mut a, "AndroidManifest.xml"), manifest);

    let (rebuilt, _) = a.rebuild().unwrap();
    let mut b = Apk::new(Cursor::new(&rebuilt)).unwrap();
    assert!(text(&mut b, "AndroidManifest.xml").contains(r#"package="com.other""#));
    assert!(text(&mut b, "res/values/strings.xml").contains(">Hi<"));
    assert_eq!(
        b.file("res/raw/note.txt").unwrap().as_deref(),
        Some(&b"note"[..])
    );
    assert_eq!(b.read("assets/x"), Some(vec![0]));
    assert!(b.file("res/values-night/strings.xml").unwrap().is_none());
}
