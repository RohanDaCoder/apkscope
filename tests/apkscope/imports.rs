use super::*;

fn m(full: &str) -> String {
    format!("{OPEN}{full}{CLOSE}")
}

#[test]
fn framework_paths_nest_on_dollar() {
    assert_eq!(
        source_path("Landroid/os/Parcelable$Creator;"),
        "android.os/Parcelable.Creator"
    );
}

#[test]
fn imports_and_clashes() {
    let text = format!(
        "package a.b;\n\nclass Foo {{\n    {} x; {} y; {} z; {} w; {} q;\n}}\n",
        m("java.util/List"),
        m("a.b/Foo.Inner"),
        m("c.d/Item"),
        m("e.f/Item"),
        m("java.lang/String")
    );
    let out = resolve(&text, "a.b", "a.b/Foo");
    assert!(
        out.starts_with("package a.b;\n\nimport c.d.Item;\nimport java.util.List;\n\nclass Foo"),
        "{out}"
    );
    // one Item is imported, the other stays fully qualified; nested and java.lang need no import
    assert!(
        out.contains("List x; Inner y; Item z; e.f.Item w; String q;"),
        "{out}"
    );
}

#[test]
fn declared_nested_name_shadows_supertype() {
    DECLARED.get_or_init(|| {
        HashMap::from([(
            crate::dex::name_hash("a.b/Foo"),
            vec!["Foo".into(), "Factory".into()].into_boxed_slice(),
        )])
    });
    // `class Factory implements Factory` would be cyclic: the supertype must stay qualified
    let text = format!(
        "package a.b;

class Foo {{
    static class Factory implements {} {{}}
}}
",
        m("c.d/Bar.Factory")
    );
    let out = resolve(&text, "a.b", "a.b/Foo");
    assert!(
        out.contains("implements c.d.Bar.Factory {}") && !out.contains("import"),
        "{out}"
    );
}
