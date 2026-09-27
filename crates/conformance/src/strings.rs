//! Section `strings`: surrogates and invalid UTF-8 inside JSON strings.
//!
//! Each input is decoded into `String` and `Cow<str>` (owned and borrowed)
//! and `&str` (borrowed), through both arrivals, and must agree with
//! serde_json. The one exemption is borrowing itself: a `&str` target that
//! one side accepts and the other refuses as a wrong shape (422) measures
//! whether the backend can hand out a slice, which the non-gating probe
//! below records instead.

use std::borrow::Cow;

use codecs::backend::serde_json::SerdeJson;
use codecs::body::Arrival;
use codecs::{Backend, Body};
use serde_json::{Map, Value, json};

use crate::outcome::{ARRIVALS, Decode, Outcome, Str, Target, Typed, agree, borrowed, guarded, owned};
use crate::record::Section;

/// `Cow<'de, str>`.
#[derive(Debug)]
pub struct CowStr;

impl Target for CowStr {
    type Out<'de> = Cow<'de, str>;
    fn view(out: &Cow<'_, str>) -> Value {
        Value::String(out.to_string())
    }
}

/// The inputs, each a complete JSON string.
#[must_use]
pub fn inputs() -> Vec<(&'static str, Vec<u8>)> {
    let quoted = |inner: &[u8]| [b"\"", inner, b"\""].concat();
    vec![
        ("lone-high-surrogate", quoted(br"\ud800")),
        ("lone-low-surrogate", quoted(br"\udc00")),
        ("high-then-non-low", quoted(br"\ud800A")),
        ("high-then-escaped-non-low", quoted(br"\ud800A")),
        ("high-then-high", quoted(br"\ud800\ud800")),
        ("pair-raw", quoted("😀".as_bytes())),
        ("pair-escaped", quoted(br"\ud83d\ude00")),
        ("utf8-ff", quoted(&[b'a', 0xFF, b'b'])),
        ("utf8-overlong", quoted(&[0xC0, 0xAF])),
        ("utf8-truncated", quoted(&[0xE2, 0x82])),
        ("utf8-encoded-surrogate", quoted(&[0xED, 0xA0, 0x80])),
        ("ascii-unescaped", quoted(b"abc")),
        ("ascii-escaped", quoted(br"a\nb")),
    ]
}

/// Whether a `&str` disagreement is only about borrowing.
fn borrow_capability(reference: &Outcome, backend: &Outcome) -> bool {
    match (reference, backend) {
        (Outcome::Accept(_), Outcome::Reject(f)) | (Outcome::Reject(f), Outcome::Accept(_)) => f.status() == 422,
        _ => false,
    }
}

fn paths<B: Backend>() -> [(&'static str, Decode, Decode); 5] {
    [
        (
            "string/owned",
            owned::<B, Typed<String>>,
            owned::<SerdeJson, Typed<String>>,
        ),
        (
            "string/borrowed",
            borrowed::<B, Typed<String>>,
            borrowed::<SerdeJson, Typed<String>>,
        ),
        ("cow/owned", owned::<B, CowStr>, owned::<SerdeJson, CowStr>),
        ("cow/borrowed", borrowed::<B, CowStr>, borrowed::<SerdeJson, CowStr>),
        ("str/borrowed", borrowed::<B, Str>, borrowed::<SerdeJson, Str>),
    ]
}

/// Whether a backend can hand out `&str` for an unescaped and an escaped
/// string. (A bare `Cow<str>` is not probed: serde's impl never borrows.)
fn borrow_probe<B: Backend>(arrival: Arrival) -> Value {
    let str_ok = |input: &[u8]| {
        let (bytes, _guard) = arrival.materialize(input);
        let mut body = Body::new(bytes);
        matches!(guarded(|| B::decode_borrowed::<&str>(&mut body).is_ok()), Ok(true))
    };
    json!({
        "str_unescaped": str_ok(br#""abc""#),
        "str_escaped": str_ok(br#""a\nb""#),
    })
}

/// Run the section.
#[must_use]
pub fn run<B: Backend>() -> Section {
    let mut section = Section::new("strings");
    for (input_name, input) in inputs() {
        for (path, backend, reference) in paths::<B>() {
            for arrival in ARRIVALS {
                let name = format!("{input_name}#{path}#{}", arrival.name());
                let r = reference(&input, arrival);
                let b = backend(&input, arrival);
                section.case();
                if agree(&r, &b) {
                    continue;
                }
                section.differ(&name, r.describe(), b.describe());
                if path.starts_with("str/") && borrow_capability(&r, &b) {
                    section.note(format!(
                        "{name}: borrowing differs from serde_json (see data.borrow_probe)"
                    ));
                } else {
                    section.fail(&name, "outcome differs from serde_json");
                }
            }
        }
    }
    let mut probe = Map::new();
    for arrival in ARRIVALS {
        probe.insert(
            arrival.name().to_owned(),
            json!({ "backend": borrow_probe::<B>(arrival), "reference": borrow_probe::<SerdeJson>(arrival) }),
        );
    }
    section.data("borrow_probe", Value::Object(probe));
    section
}
