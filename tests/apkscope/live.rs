use super::*;

#[test]
fn events() {
    let mut ev = Events::default();
    ev.push(parse(
        "\u{1}{\"k\":\"call\",\"m\":\"a.B.c\",\"v\":[\"1\",\"x\"],\"tid\":7,\"ts\":0}",
        false,
    ));
    ev.push(parse("Spawned `x`.", true));
    assert_eq!(line(&ev.list[0]), "#0 t7 call   a.B.c (1, x)");
    assert_eq!(ev.list[1]["k"], "stderr");
    assert!(matches(&ev.list[0], "B.c") && !matches(&ev.list[0], "zzz"));
}

#[test]
fn dump_helpers() {
    assert_eq!(unhex("6465780aff").collect::<Vec<_>>(), b"dex\n\xff");
    assert_eq!(script_name("assets/main/index.f5b59.js"), "index.f5b59.js");
    assert_eq!(script_name("a/b c?"), "b_c_.js");
    assert_eq!(script_name(""), "script.js");
}
