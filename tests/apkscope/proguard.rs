use super::*;

const MAPPING: &str = concat!(
    "# ProGuard, R8 and AGP: the same file\n",
    "com.example.app.MainActivity -> com.example.app.a:\n",
    "    int status -> a\n",
    "    void onCreate(android.os.Bundle) -> a\n",
    "    android.view.View findViewById(int) -> b:11:22\n",
    "com.example.app.User$Name -> a.b:\n",
    "    java.lang.String first -> a\n",
);

#[test]
fn parses_classes_and_members() {
    let p = parse(MAPPING);
    assert_eq!(p.classes.len(), 2);
    assert_eq!(
        p.classes["Lcom/example/app/a;"],
        "com.example.app.MainActivity"
    );
    assert_eq!(p.classes["La/b;"], "com.example.app.User$Name");
    // members keyed (class, obfuscated-member); methods keep their
    // signature in the key form, fields do not
    assert_eq!(
        p.members
            .iter()
            .find(|((c, k), _)| c == "Lcom/example/app/a;" && k == "Lcom/example/app/a;->a")
            .map(|(_, v)| v.as_str()),
        Some("status")
    );
    assert_eq!(
        p.members
            .iter()
            .find(|((c, k), _)| c == "Lcom/example/app/a;" && k == "Lcom/example/app/a;->a()")
            .map(|(_, v)| v.as_str()),
        Some("onCreate")
    );
    assert_eq!(
        p.members
            .iter()
            .find(|((c, k), _)| c == "Lcom/example/app/a;" && k == "Lcom/example/app/a;->b()")
            .map(|(_, v)| v.as_str()),
        Some("findViewById")
    );
    assert_eq!(
        p.members
            .iter()
            .find(|((c, k), _)| c == "La/b;" && k == "La/b;->a")
            .map(|(_, v)| v.as_str()),
        Some("first")
    );
}

#[test]
fn line_number_tails_are_stripped() {
    let p = parse(concat!("a.C -> a.a:\n", "    void run() -> a:9:19\n",));
    // `a:9:19` must not be taken for the name; the method still maps
    assert_eq!(
        p.members
            .iter()
            .find(|((_, k), _)| k == "La/a;->a()")
            .map(|(_, v)| v.as_str()),
        Some("run")
    );
}

#[test]
fn member_before_any_class_is_ignored() {
    let p = parse(concat!("    int x -> a\n", "a.C -> a.a:\n"));
    assert!(p.members.is_empty());
    assert_eq!(p.classes.len(), 1);
}

#[test]
fn comments_and_blanks_are_skipped() {
    let p = parse("# a comment\n\na.C -> a.a:\n# another\n");
    assert_eq!(p.classes.len(), 1);
}

#[test]
fn imports_into_notes() {
    let mut n = Notes::default();
    let k = into_notes(parse(MAPPING), &mut n);
    // 2 classes + 4 members, methods and fields keyed apart by the ()
    // suffix the notes format uses
    assert_eq!(k, 2 + 4);
    assert_eq!(
        n.classes["Lcom/example/app/a;"],
        "Lcom/example/app/MainActivity;"
    );
    assert_eq!(n.classes["La/b;"], "Lcom/example/app/User$Name;");
    assert_eq!(n.members["Lcom/example/app/a;->a"], "status");
    assert_eq!(n.members["Lcom/example/app/a;->a()"], "onCreate");
    assert_eq!(n.members["Lcom/example/app/a;->b()"], "findViewById");
    assert_eq!(n.members["La/b;->a"], "first");
}

#[test]
fn reimport_is_stable() {
    let mut n = Notes::default();
    into_notes(parse(MAPPING), &mut n);
    // re-importing the same file leaves the same names and counts 0
    let snapshot = (n.classes.clone(), n.members.clone());
    assert_eq!(into_notes(parse(MAPPING), &mut n), 0);
    assert_eq!((n.classes.clone(), n.members.clone()), snapshot);
}

#[test]
fn member_key_drops_signature() {
    // two overloads of one obfuscated name collapse to one note entry;
    // the file's last line wins, like a rename typed twice
    let p = parse(concat!(
        "a.C -> a.a:\n",
        "    void run() -> a\n",
        "    void run(int) -> a\n",
    ));
    let mut n = Notes::default();
    assert_eq!(into_notes(p, &mut n), 1 + 1); // class + one member
    assert_eq!(n.members["La/a;->a()"], "run");
}
