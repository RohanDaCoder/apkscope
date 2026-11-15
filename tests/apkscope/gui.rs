use super::*;

#[test]
fn highlight_splits_tokens_and_marks_line() {
    let p = Theme::Dark.palette();
    let job = highlight(
        "int x = 1; // hi\nString s = \"a\\\"b\";",
        &p,
        13.0,
        Some(2),
        None,
    );
    let parts: Vec<&str> = job
        .sections
        .iter()
        .map(|s| &job.text[s.byte_range.clone()])
        .collect();
    assert!(
        parts.contains(&"int")
            && parts.contains(&"// hi")
            && parts.contains(&"\"a\\\"b\"")
            && parts.contains(&"1"),
        "{parts:?}"
    );
    // only line 2 gets a background
    let marked: String = job
        .sections
        .iter()
        .filter(|s| s.format.background != Color32::TRANSPARENT)
        .map(|s| &job.text[s.byte_range.clone()])
        .collect();
    assert_eq!(marked, "String s = \"a\\\"b\";");
}

#[test]
fn tree_nests_packages() {
    let names = ["a.b.C".to_string(), "a.D".to_string(), "E".to_string()];
    let t = tree(names.iter());
    assert_eq!(t.leaves, vec![("E".to_string(), "E".to_string())]);
    assert_eq!(t.dirs["a"].leaves[0].0, "D");
    assert_eq!(t.dirs["a"].dirs["b"].leaves[0].1, "a.b.C");
}

#[test]
fn finds_member_under_cursor() {
    let java = "package a;\n\npublic class Outer {\n    private int count;\n\n    /* WARNING: 1 jump could not be structured. */\n    public static String a(int i) {\n        int x = i;\n        return null;\n    }\n\n    static class In {\n        void run() {\n        }\n    }\n}\n";
    assert_eq!(member_at(java, 2), Some((vec!["Outer".into()], None)));
    assert_eq!(
        member_at(java, 3),
        Some((vec!["Outer".into()], Some((false, "count".into()))))
    );
    // the WARNING comment points at the method below it
    assert_eq!(
        member_at(java, 5),
        Some((vec!["Outer".into()], Some((true, "a".into()))))
    );
    assert_eq!(member_at(java, 7), None);
    assert_eq!(
        member_at(java, 12),
        Some((
            vec!["Outer".into(), "In".into()],
            Some((true, "run".into()))
        ))
    );
}

#[test]
fn words_declarations_and_call_lines() {
    let java = "public class A {
    void run(int x) {
        B b = new B();
        b.go(x);
        helper();
    }

    void helper() {
        go();
    }
}
";
    assert_eq!(word_at(java, 2, 18), Some(("B".into(), true, true, None)));
    assert_eq!(
        word_at(java, 3, 11),
        Some(("go".into(), true, false, Some("b".into())))
    );
    assert_eq!(word_at(java, 3, 8), Some(("b".into(), false, false, None)));
    assert_eq!(find_decl(java, "helper", Some(0)), Some(8));
    assert_eq!(find_decl(java, "run", Some(1)), Some(2));
    // the call inside `run`, not the declaration and not the `go()` in helper
    let t = Target {
        decl: Some(("run".into(), Some(1))),
        then: Some("go(".into()),
    };
    assert_eq!(locate(java, &t), Some(4));
    let t = Target {
        decl: Some(("helper".into(), None)),
        then: Some("go(".into()),
    };
    assert_eq!(locate(java, &t), Some(9));
    assert_eq!(classes_around(java, 3), vec!["A".to_string()]);
}

#[test]
fn short_names_and_find() {
    assert_eq!(Open::Class("a.b.Foo".into()).short(), "Foo");
    assert_eq!(Open::Res("res/layout/main.xml".into()).short(), "main.xml");
    assert_eq!(Open::Overview.short(), "Overview");
    let t = "a\nfoo\nb\nFOO";
    assert_eq!(find_next(t, "foo", 0), Some(2));
    assert_eq!(find_next(t, "foo", 2), Some(4));
    // wraps around
    assert_eq!(find_next(t, "foo", 4), Some(2));
    assert_eq!(find_next(t, "zzz", 0), None);
}
