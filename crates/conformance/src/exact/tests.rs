use super::{Node, numbers_equal, parse, semantically_equal};

#[test]
fn numbers_compare_by_value() {
    assert!(numbers_equal("1.50", "1.5"));
    assert!(numbers_equal("1e2", "100.0"));
    assert!(numbers_equal("-0", "0"));
    assert!(!numbers_equal("-0.0", "0.0"));
    assert!(!numbers_equal("0.1", "0.2"));
    assert!(numbers_equal("18446744073709551616", "18446744073709551616"));
    assert!(!numbers_equal("9007199254740993", "9007199254740992"));
    let huge = format!("1{}", "0".repeat(60));
    assert!(numbers_equal(&huge, &format!("0{huge}")));
}

#[test]
fn strings_compare_after_unescaping() {
    assert_eq!(semantically_equal(br#""\u00e9\/""#, "\"é/\"".as_bytes()), Ok(true));
    assert_eq!(semantically_equal(br#""\ud83d\ude00""#, "\"😀\"".as_bytes()), Ok(true));
    assert_eq!(semantically_equal(br#"{"a":1,"b":2}"#, br#"{"b":2,"a":1}"#), Ok(false));
}

#[test]
fn rejects_what_rfc_8259_rejects() {
    for bad in [
        &b"[1,]"[..],
        b"01",
        b"1.",
        b"\"\\ud800\"",
        b"\"\x01\"",
        b"NaN",
        b"[1] x",
        b"\"\xff\"",
    ] {
        assert!(parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
    }
    assert_eq!(
        parse(b" [true, null] "),
        Ok(Node::Array(vec![Node::Bool(true), Node::Null]))
    );
}
