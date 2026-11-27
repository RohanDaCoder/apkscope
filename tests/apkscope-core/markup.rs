use super::*;

/// An attribute twice on one element is a typo: refuse it, do not quietly take the first.
/// (Patch bundles lean on this — a `<target>` with `package` twice must not check only one.)
#[test]
fn refuses_an_attribute_written_twice() {
    assert_eq!(
        parse_xml("<e a=\"1\" a=\"2\"/>").unwrap_err().to_string(),
        "line 1: <e> has attribute a twice"
    );
    // once is fine, and different attributes are fine
    parse_xml("<e a=\"1\" b=\"2\"/>").unwrap();
}

#[test]
fn reads_attributes_and_text() {
    let e = parse_xml("<e a=\"1\">text <b c=\"x\"/> tail</e>").unwrap();
    assert_eq!(e.name, "e");
    assert_eq!(e.attr("a"), Some("1"));
    assert_eq!(e.text(), "text  tail");
    let b = e.elements().next().unwrap();
    assert_eq!((b.name.as_str(), b.attr("c")), ("b", Some("x")));
}
