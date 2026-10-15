//! Is a result the value it should be?

use std::collections::BTreeMap;

use codecs::{Class, Failure};
use payloads::{Expect, Workload};
use serde::Serialize;
use serde_json::Value;

/// A result, judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Identical, floats bit for bit.
    Exact,
    /// Identical except floats within `max_ulps` of the truth.
    Inexact { floats: u64, max_ulps: u64 },
    /// A different value.
    Wrong(String),
    /// The backend refused the input.
    Rejected(String),
}

impl Verdict {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Inexact { .. } => "inexact",
            Self::Wrong(_) => "wrong",
            Self::Rejected(_) => "rejected",
        }
    }

    /// Whether the result is fit to be measured.
    pub fn measurable(&self) -> bool {
        matches!(self, Self::Exact | Self::Inexact { .. })
    }
}

/// Floats further apart than this are a different value, not a rounding.
const ROUNDING_ULPS: u64 = 4;

fn ulps(a: f64, b: f64) -> u64 {
    let key = |x: f64| {
        let bits = x.to_bits() as i64;
        // Negative floats map below zero in bit order, and -0.0 sits one
        // step below +0.0: they are different values.
        if bits < 0 { -(bits & i64::MAX) - 1 } else { bits }
    };
    key(a).abs_diff(key(b))
}

/// Floats seen inexact so far, and the furthest apart.
#[derive(Default)]
struct Tally {
    floats: u64,
    max_ulps: u64,
}

impl Tally {
    /// Two floats: equal by bits, or a rounding apart (counted), or a
    /// different value.
    fn floats(&mut self, got: f64, expected: f64) -> Result<(), String> {
        if got.to_bits() == expected.to_bits() {
            return Ok(());
        }
        let distance = ulps(got, expected);
        if distance > ROUNDING_ULPS {
            return Err(format!("{got:e} != {expected:e}"));
        }
        self.floats += 1;
        self.max_ulps = self.max_ulps.max(distance);
        Ok(())
    }

    fn verdict(self, result: Result<(), String>) -> Verdict {
        match result {
            Err(path) => Verdict::Wrong(path),
            Ok(()) if self.floats == 0 => Verdict::Exact,
            Ok(()) => Verdict::Inexact {
                floats: self.floats,
                max_ulps: self.max_ulps,
            },
        }
    }
}

/// Judge a decode of workload `W`'s bytes against what `W` expects.
///
/// For [`Expect::Value`], a value is compared with [`Workload::value`] and a
/// refusal is `Rejected`. For [`Expect::Reject`], the refusal *is* the
/// result: refused with the expected status is exact; accepted, or refused
/// with another status, is wrong, with the status given in the detail, so a
/// backend that would answer a client wrongly is never measured. A backend
/// without the operation (`Unsupported`) is `Rejected` either way: it
/// answered nothing, rightly or wrongly.
pub fn judge_decode<W: Workload, T: Serialize>(result: Result<T, Failure>) -> Verdict {
    match (W::EXPECT, result) {
        (_, Err(failure)) if failure.class == Class::Unsupported => Verdict::Rejected(failure.to_string()),
        (Expect::Value, Ok(value)) => compare(&to_value(&value), &to_value(&W::value())),
        (Expect::Value, Err(failure)) => Verdict::Rejected(failure.to_string()),
        (Expect::Reject(status), Ok(_)) => Verdict::Wrong(format!("accepted; expected a refusal with status {status}")),
        (Expect::Reject(status), Err(failure)) if failure.status() == status => Verdict::Exact,
        (Expect::Reject(status), Err(failure)) => {
            Verdict::Wrong(format!("answered {}, expected {status}: {failure}", failure.status()))
        }
    }
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|e| unreachable!("a decoded value re-serializes: {e}"))
}

/// Compare `got` with `expected`.
pub fn compare(got: &Value, expected: &Value) -> Verdict {
    let mut tally = Tally::default();
    let result = walk(got, expected, &mut tally, &mut String::from("$"));
    tally.verdict(result)
}

fn walk(got: &Value, expected: &Value, tally: &mut Tally, path: &mut String) -> Result<(), String> {
    match (got, expected) {
        (Value::Number(g), Value::Number(e)) if g.is_f64() || e.is_f64() => tally
            .floats(g.as_f64().unwrap_or(f64::NAN), e.as_f64().unwrap_or(f64::NAN))
            .map_err(|difference| format!("{path}: {difference}")),
        (Value::Array(g), Value::Array(e)) if g.len() == e.len() => {
            for (i, (g, e)) in g.iter().zip(e).enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                walk(g, e, tally, path)?;
                path.truncate(len);
            }
            Ok(())
        }
        (Value::Object(g), Value::Object(e)) if g.len() == e.len() => {
            for (key, e) in e {
                let len = path.len();
                path.push_str(&format!(".{key}"));
                walk(g.get(key).ok_or_else(|| format!("{path}: missing"))?, e, tally, path)?;
                path.truncate(len);
            }
            Ok(())
        }
        (g, e) if g == e => Ok(()),
        _ => Err(format!("{path}: {got} != {expected}")),
    }
}

/// Judge an encoder's output for workload `W` against its canonical
/// encoding, in three steps:
///
/// 1. the same bytes are exact;
/// 2. otherwise the output is decoded by serde_json into `W::Owned` and
///    compared with [`compare`], so each number is read at its field's type:
///    an `f32` written widened (`2.38303598756323e-24` for serde_json's
///    `2.383036e-24`) reads back as the same `f32` and is exact;
/// 3. only where serde_json refuses on its recursion limit (the nested
///    workloads) is the output compared structurally, by [`compare_json`].
///
/// Step 2 reads what the type declares, so a member the type does not have
/// would be skipped; a `Serialize` of `W::Owned` has no way to write one.
pub fn judge_encoding<W: Workload>(got: &[u8], canonical: &[u8]) -> Verdict {
    if got == canonical {
        return Verdict::Exact;
    }
    match serde_json::from_slice::<W::Owned>(got) {
        Ok(typed) => match (serde_json::to_value(&typed), serde_json::to_value(W::value())) {
            (Ok(got), Ok(expected)) => compare(&got, &expected),
            (Err(e), _) | (_, Err(e)) => Verdict::Wrong(format!("not representable as a value: {e}")),
        },
        Err(e) if e.to_string().starts_with("recursion limit exceeded") => compare_json(got, canonical),
        Err(e) => Verdict::Wrong(format!("output does not decode as {}: {e}", W::NAME)),
    }
}

/// Compare an encoder's output with the canonical encoding: the same bytes
/// are exact; otherwise both are parsed and compared as [`compare`] compares
/// values, by [`Document`], which has no depth limit and never recurses, so
/// `nested-10000` is judged rather than refused at serde_json's 128 levels
/// or overflowing the stack.
pub fn compare_json(got: &[u8], expected: &[u8]) -> Verdict {
    if got == expected {
        return Verdict::Exact;
    }
    let got = match Document::parse(got) {
        Ok(document) => document,
        Err(error) => return Verdict::Wrong(format!("output is not JSON: {error}")),
    };
    let expected = Document::parse(expected).unwrap_or_else(|e| unreachable!("the canonical encoding is JSON: {e}"));
    let mut tally = Tally::default();
    let result = got.compare(&expected, &mut tally);
    tally.verdict(result)
}

/// A number as serde_json's `Value` holds it: an integer if the text is one
/// that fits 64 bits, a float otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Number {
    Int(i128),
    Float(f64),
}

impl Number {
    fn as_f64(self) -> f64 {
        match self {
            Self::Int(i) => i as f64,
            Self::Float(f) => f,
        }
    }
}

/// One JSON value; containers hold their children's indices.
#[derive(Debug)]
enum Node {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<usize>),
    /// Duplicate keys keep the last value, as `Value` does.
    Object(BTreeMap<String, usize>),
}

/// How a node is reached from its parent.
#[derive(Debug)]
enum Step {
    Index(usize),
    Key(String),
}

/// A parsed JSON document, flat: nodes in an arena, so building, comparing
/// and dropping it take no stack proportional to its depth.
#[derive(Debug)]
struct Document {
    nodes: Vec<Node>,
    /// Each node's parent and the step from it; `None` for the root.
    parents: Vec<Option<(usize, Step)>>,
}

/// A container still being parsed.
enum Open {
    Array(usize),
    /// The object, and the key whose value comes next.
    Object(usize, String),
}

impl Document {
    /// Parse RFC 8259 JSON: one value, surrounded only by whitespace.
    fn parse(input: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(input).map_err(|e| format!("invalid UTF-8: {e}"))?;
        let mut lexer = Lexer {
            bytes: text.as_bytes(),
            pos: 0,
        };
        let mut document = Self {
            nodes: Vec::new(),
            parents: Vec::new(),
        };
        let mut open: Vec<Open> = Vec::new();
        loop {
            // A value starts here.
            lexer.whitespace();
            let mut value = match lexer.next()? {
                b'{' => {
                    let object = document.push(Node::Object(BTreeMap::new()));
                    lexer.whitespace();
                    if lexer.eat(b'}') {
                        object
                    } else {
                        open.push(Open::Object(object, lexer.key()?));
                        continue;
                    }
                }
                b'[' => {
                    let array = document.push(Node::Array(Vec::new()));
                    lexer.whitespace();
                    if lexer.eat(b']') {
                        array
                    } else {
                        open.push(Open::Array(array));
                        continue;
                    }
                }
                b'"' => {
                    let string = lexer.string()?;
                    document.push(Node::String(string))
                }
                b't' => document.push(lexer.literal("rue", Node::Bool(true))?),
                b'f' => document.push(lexer.literal("alse", Node::Bool(false))?),
                b'n' => document.push(lexer.literal("ull", Node::Null)?),
                b'-' | b'0'..=b'9' => {
                    lexer.pos -= 1;
                    let number = lexer.number()?;
                    document.push(Node::Number(number))
                }
                other => return Err(lexer.unexpected(other)),
            };
            // A value is complete: attach it, closing every container it
            // completes, until one wants another value or the document ends.
            loop {
                let Some(container) = open.last_mut() else {
                    lexer.whitespace();
                    return match lexer.bytes.get(lexer.pos) {
                        None => Ok(document),
                        Some(&other) => Err(format!("trailing {}", lexer.unexpected(other))),
                    };
                };
                lexer.whitespace();
                match container {
                    Open::Array(array) => {
                        let array = *array;
                        if let Node::Array(items) = &mut document.nodes[array] {
                            document.parents[value] = Some((array, Step::Index(items.len())));
                            items.push(value);
                        }
                        match lexer.next()? {
                            b',' => break,
                            b']' => {
                                open.pop();
                                value = array;
                            }
                            other => return Err(lexer.unexpected(other)),
                        }
                    }
                    Open::Object(object, key) => {
                        let object = *object;
                        if let Node::Object(members) = &mut document.nodes[object] {
                            members.insert(key.clone(), value);
                        }
                        document.parents[value] = Some((object, Step::Key(std::mem::take(key))));
                        match lexer.next()? {
                            b',' => {
                                lexer.whitespace();
                                *key = lexer.key()?;
                                break;
                            }
                            b'}' => {
                                open.pop();
                                value = object;
                            }
                            other => return Err(lexer.unexpected(other)),
                        }
                    }
                }
            }
        }
    }

    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.parents.push(None);
        self.nodes.len() - 1
    }

    /// The node's path from the root, as [`walk`] writes it.
    fn path(&self, mut node: usize) -> String {
        let mut steps = Vec::new();
        while let Some((parent, step)) = &self.parents[node] {
            steps.push(step);
            node = *parent;
        }
        let mut path = String::from("$");
        for step in steps.into_iter().rev() {
            match step {
                Step::Index(i) => path.push_str(&format!("[{i}]")),
                Step::Key(key) => path.push_str(&format!(".{key}")),
            }
        }
        path
    }

    /// A node, abbreviated for a message.
    fn show(&self, node: usize) -> String {
        match &self.nodes[node] {
            Node::Null => "null".to_owned(),
            Node::Bool(b) => b.to_string(),
            Node::Number(Number::Int(i)) => i.to_string(),
            Node::Number(Number::Float(f)) => format!("{f:e}"),
            Node::String(s) => format!("{s:?}"),
            Node::Array(items) => format!("[{} items]", items.len()),
            Node::Object(members) => format!("{{{} members}}", members.len()),
        }
    }

    /// Compare with `expected`, root to leaves on an explicit stack, in
    /// document order so the first difference reported is the first one.
    fn compare(&self, expected: &Self, tally: &mut Tally) -> Result<(), String> {
        let mut pending = vec![(0, 0)];
        while let Some((g, e)) = pending.pop() {
            match (&self.nodes[g], &expected.nodes[e]) {
                (Node::Number(Number::Int(a)), Node::Number(Number::Int(b))) if a == b => {}
                (Node::Number(gn), Node::Number(en))
                    if matches!(gn, Number::Float(_)) || matches!(en, Number::Float(_)) =>
                {
                    tally
                        .floats(gn.as_f64(), en.as_f64())
                        .map_err(|difference| format!("{}: {difference}", expected.path(e)))?;
                }
                (Node::Array(gs), Node::Array(es)) if gs.len() == es.len() => {
                    pending.extend(gs.iter().copied().zip(es.iter().copied()).rev());
                }
                (Node::Object(gm), Node::Object(em)) if gm.len() == em.len() => {
                    let mut pairs = Vec::with_capacity(em.len());
                    for (key, &child) in em {
                        let &got = gm
                            .get(key)
                            .ok_or_else(|| format!("{}.{key}: missing", expected.path(e)))?;
                        pairs.push((got, child));
                    }
                    pending.extend(pairs.into_iter().rev());
                }
                (Node::Null, Node::Null) => {}
                (Node::Bool(a), Node::Bool(b)) if a == b => {}
                (Node::String(a), Node::String(b)) if a == b => {}
                _ => {
                    return Err(format!(
                        "{}: {} != {}",
                        expected.path(e),
                        self.show(g),
                        expected.show(e)
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A cursor over a document's bytes.
struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn whitespace(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn next(&mut self) -> Result<u8, String> {
        let byte = *self.bytes.get(self.pos).ok_or("unexpected end of input")?;
        self.pos += 1;
        Ok(byte)
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.bytes.get(self.pos) == Some(&byte);
        self.pos += usize::from(found);
        found
    }

    fn unexpected(&self, byte: u8) -> String {
        format!(
            "unexpected {:?} at byte {}",
            char::from(byte),
            self.pos.saturating_sub(1)
        )
    }

    fn literal(&mut self, rest: &str, node: Node) -> Result<Node, String> {
        if self.bytes[self.pos..].starts_with(rest.as_bytes()) {
            self.pos += rest.len();
            Ok(node)
        } else {
            Err(format!("invalid literal at byte {}", self.pos - 1))
        }
    }

    /// A member name and its colon.
    fn key(&mut self) -> Result<String, String> {
        match self.next()? {
            b'"' => {}
            other => return Err(self.unexpected(other)),
        }
        let key = self.string()?;
        self.whitespace();
        match self.next()? {
            b':' => Ok(key),
            other => Err(self.unexpected(other)),
        }
    }

    /// The rest of a string whose opening quote was consumed.
    fn string(&mut self) -> Result<String, String> {
        let mut out = String::new();
        loop {
            let start = self.pos;
            while let Some(&b) = self.bytes.get(self.pos) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            // The input is UTF-8 and the run stops only at ASCII bytes.
            out.push_str(std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|e| e.to_string())?);
            match self.next()? {
                b'"' => return Ok(out),
                b'\\' => {
                    let escaped = match self.next()? {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode_escape()?,
                        other => return Err(format!("invalid escape {:?}", char::from(other))),
                    };
                    out.push(escaped);
                }
                _ => return Err(format!("control character in a string at byte {}", self.pos - 1)),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or("unexpected end of input")?;
        let digits = std::str::from_utf8(digits).map_err(|e| e.to_string())?;
        let unit = u32::from_str_radix(digits, 16).map_err(|_| format!("invalid \\u escape {digits:?}"))?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("invalid \\u escape {digits:?}"));
        }
        self.pos += 4;
        Ok(unit)
    }

    /// A `\u` escape after its `\u`, taking the low half of a surrogate pair.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let unit = self.hex4()?;
        let code = match unit {
            0xd800..=0xdbff => {
                if !self.bytes[self.pos..].starts_with(b"\\u") {
                    return Err("lone leading surrogate".to_owned());
                }
                self.pos += 2;
                let low = self.hex4()?;
                if !(0xdc00..=0xdfff).contains(&low) {
                    return Err("lone leading surrogate".to_owned());
                }
                0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00)
            }
            0xdc00..=0xdfff => return Err("lone trailing surrogate".to_owned()),
            _ => unit,
        };
        char::from_u32(code).ok_or_else(|| format!("invalid code point {code:x}"))
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`, as serde_json reads
    /// it: an integer if it has neither fraction nor exponent and fits in
    /// `u64` or `i64`, else a float, which must be finite.
    fn number(&mut self) -> Result<Number, String> {
        let start = self.pos;
        let digits = |lexer: &mut Self| {
            let from = lexer.pos;
            while lexer.bytes.get(lexer.pos).is_some_and(u8::is_ascii_digit) {
                lexer.pos += 1;
            }
            lexer.pos - from
        };
        self.eat(b'-');
        let int_start = self.pos;
        let int_digits = digits(self);
        if int_digits == 0 || (int_digits > 1 && self.bytes[int_start] == b'0') {
            return Err(format!("invalid number at byte {start}"));
        }
        let mut integer = true;
        if self.eat(b'.') {
            integer = false;
            if digits(self) == 0 {
                return Err(format!("invalid number at byte {start}"));
            }
        }
        if self.eat(b'e') || self.eat(b'E') {
            integer = false;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if digits(self) == 0 {
                return Err(format!("invalid number at byte {start}"));
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|e| e.to_string())?;
        if integer
            && let Ok(value) = text.parse::<i128>()
            && (i128::from(i64::MIN)..=i128::from(u64::MAX)).contains(&value)
        {
            return Ok(Number::Int(value));
        }
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(Number::Float(value)),
            _ => Err(format!("number out of range at byte {start}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn floats_one_ulp_apart_are_inexact_not_wrong() {
        let a = 0.1f64;
        let b = f64::from_bits(a.to_bits() + 1);
        assert_eq!(
            compare(&json!([a]), &json!([b])),
            Verdict::Inexact { floats: 1, max_ulps: 1 }
        );
    }

    #[test]
    fn a_different_string_is_wrong() {
        assert!(matches!(
            compare(&json!({"a": "x"}), &json!({"a": "y"})),
            Verdict::Wrong(_)
        ));
    }

    #[test]
    fn a_missing_key_is_wrong() {
        assert!(matches!(compare(&json!({"a": 1}), &json!({"b": 1})), Verdict::Wrong(_)));
    }

    use codecs::{Class, Failure};
    use payloads::kynos::{Catalog, JsonLarge};
    use payloads::reject::{RejectShape, RejectTruncated};

    #[test]
    fn a_rejection_is_judged_by_its_status() {
        let data = || Err::<Catalog, _>(Failure::new(Class::Data, "invalid type"));
        let eof = || Err::<Catalog, _>(Failure::new(Class::Eof, "EOF while parsing"));
        let syntax = || Err::<Catalog, _>(Failure::new(Class::Syntax, "expected value"));
        assert_eq!(judge_decode::<RejectShape, _>(data()), Verdict::Exact);
        assert_eq!(judge_decode::<RejectTruncated, _>(eof()), Verdict::Exact);
        assert_eq!(judge_decode::<RejectTruncated, _>(syntax()), Verdict::Exact);
        assert_eq!(
            judge_decode::<RejectShape, _>(syntax()),
            Verdict::Wrong("answered 400, expected 422: Syntax: expected value".to_owned())
        );
        assert_eq!(
            judge_decode::<RejectTruncated, _>(data()),
            Verdict::Wrong("answered 422, expected 400: Data: invalid type".to_owned())
        );
        assert_eq!(
            judge_decode::<RejectShape, _>(Ok(JsonLarge::value())),
            Verdict::Wrong("accepted; expected a refusal with status 422".to_owned())
        );
    }

    #[test]
    fn an_unsupported_operation_is_no_answer() {
        for verdict in [
            judge_decode::<RejectTruncated, Catalog>(Err(Failure::unsupported())),
            judge_decode::<JsonLarge, Catalog>(Err(Failure::unsupported())),
        ] {
            assert!(matches!(verdict, Verdict::Rejected(_)), "{verdict:?}");
        }
    }

    #[test]
    fn a_value_workload_is_judged_by_its_value() {
        assert_eq!(judge_decode::<JsonLarge, _>(Ok(JsonLarge::value())), Verdict::Exact);
        let mut other = JsonLarge::value();
        other.revision += 1;
        assert!(matches!(judge_decode::<JsonLarge, _>(Ok(other)), Verdict::Wrong(_)));
        assert!(matches!(
            judge_decode::<JsonLarge, Catalog>(Err(Failure::new(Class::Data, "x"))),
            Verdict::Rejected(_)
        ));
    }

    #[test]
    fn ulps_cross_zero() {
        assert_eq!(ulps(0.0, -0.0), 1);
        assert_eq!(ulps(f64::from_bits(1), -f64::from_bits(1)), 3);
    }

    /// `{"d":{"d":...{"d":null}...}}`, `depth` levels, with `pad` between
    /// tokens and `leaf` at the bottom.
    fn nested(depth: usize, pad: &str, leaf: &str) -> Vec<u8> {
        let mut out = String::new();
        for _ in 0..depth {
            out.push_str(&format!("{{{pad}\"d\"{pad}:{pad}"));
        }
        out.push_str(leaf);
        for _ in 0..depth {
            out.push_str(&format!("{pad}}}"));
        }
        out.into_bytes()
    }

    #[test]
    fn identical_bytes_are_exact_at_any_depth() {
        let deep = nested(10_000, "", "null");
        assert_eq!(compare_json(&deep, &deep), Verdict::Exact);
    }

    #[test]
    fn deep_documents_compare_structurally_without_recursion() {
        // Runs on the test thread's small stack: a recursive comparison of
        // 10 000 levels, or a recursive drop of the tree, would overflow it.
        let canonical = nested(10_000, "", "null");
        assert_eq!(compare_json(&nested(10_000, " ", "null"), &canonical), Verdict::Exact);
        let Verdict::Wrong(path) = compare_json(&nested(10_000, "", "1"), &canonical) else {
            panic!("a different leaf is wrong");
        };
        assert!(path.starts_with("$.d.d.d") && path.ends_with(".d: 1 != null"), "{path}");
        let deeper = nested(10_001, "", "null");
        assert!(matches!(compare_json(&deeper, &canonical), Verdict::Wrong(_)));
    }

    #[test]
    fn deep_arrays_compare_too() {
        let canonical = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
        let spaced = format!("{} {}", "[".repeat(10_000), "]".repeat(10_000));
        assert_eq!(compare_json(spaced.as_bytes(), canonical.as_bytes()), Verdict::Exact);
    }

    #[test]
    fn member_order_whitespace_and_escapes_do_not_matter() {
        let canonical = r#"{"a":"é/","b":[1,2]}"#.as_bytes();
        let other = br#" { "b" : [ 1 , 2 ] , "a" : "\u00e9\/" } "#;
        assert_eq!(compare_json(other, canonical), Verdict::Exact);
        let astral = r#""😀""#.as_bytes();
        assert_eq!(compare_json(br#""\ud83d\ude00""#, astral), Verdict::Exact);
    }

    #[test]
    fn numbers_compare_as_values() {
        assert_eq!(compare_json(b"[1.0,1E2]", b"[1,100]"), Verdict::Exact);
        assert_eq!(
            compare_json(b"18446744073709551615", b"18446744073709551615"),
            Verdict::Exact
        );
        assert!(matches!(compare_json(b"[2]", b"[1]"), Verdict::Wrong(_)));
        let a = 0.1f64;
        let b = f64::from_bits(a.to_bits() + 2);
        assert_eq!(
            compare_json(format!("[{a:?}]").as_bytes(), format!("[{b:?} ]").as_bytes()),
            Verdict::Inexact { floats: 1, max_ulps: 2 }
        );
        let far = f64::from_bits(a.to_bits() + 5);
        assert!(matches!(
            compare_json(format!("[{a:?}]").as_bytes(), format!("[{far:?} ]").as_bytes()),
            Verdict::Wrong(_)
        ));
    }

    #[test]
    fn duplicate_keys_keep_the_last_as_value_does() {
        assert_eq!(compare_json(br#"{"a":1,"a":2}"#, br#"{"a":2}"#), Verdict::Exact);
    }

    #[test]
    fn a_different_shape_is_wrong_with_its_path() {
        assert_eq!(
            compare_json(br#"{"a":[1,{"b":true}]}"#, br#"{"a":[1,{"b":false}]}"#),
            Verdict::Wrong("$.a[1].b: true != false".to_owned())
        );
        assert_eq!(
            compare_json(br#"{"a":[1]}"#, br#"{"a":[1,2]}"#),
            Verdict::Wrong("$.a: [1 items] != [2 items]".to_owned())
        );
        assert_eq!(
            compare_json(br#"{"b":1}"#, br#"{"a":1}"#),
            Verdict::Wrong("$.a: missing".to_owned())
        );
    }

    use payloads::families::{F32s, Nested};

    /// `values` written as simd-json writes `f32`s: widened to `f64` first.
    fn widened(values: &[f32]) -> Vec<u8> {
        let texts: Vec<String> = values.iter().map(|&f| format!("{:?}", f64::from(f))).collect();
        format!("[{}]", texts.join(",")).into_bytes()
    }

    #[test]
    fn f32s_written_widened_are_exact() {
        let canonical = F32s::bytes();
        let output = widened(&F32s::value());
        assert_ne!(output, canonical);
        // As f64s the two texts are millions of ulps apart.
        assert!(matches!(compare_json(&output, &canonical), Verdict::Wrong(_)));
        assert_eq!(judge_encoding::<F32s>(&output, &canonical), Verdict::Exact);
    }

    #[test]
    fn a_different_f32_is_wrong() {
        let mut values = F32s::value();
        values[3] = f32::from_bits(values[3].to_bits() + 1);
        let verdict = judge_encoding::<F32s>(&widened(&values), &F32s::bytes());
        assert!(
            matches!(&verdict, Verdict::Wrong(path) if path.starts_with("$[3]")),
            "{verdict:?}"
        );
    }

    #[test]
    fn past_serde_jsons_depth_limit_the_comparison_is_structural() {
        let canonical = Nested::<129>::bytes();
        let spaced = String::from_utf8(canonical.clone()).expect("UTF-8").replace(':', ": ");
        assert_eq!(
            judge_encoding::<Nested<129>>(spaced.as_bytes(), &canonical),
            Verdict::Exact
        );
        let other = String::from_utf8(canonical.clone())
            .expect("UTF-8")
            .replace("\"depth\":128", "\"depth\":7");
        assert!(matches!(
            judge_encoding::<Nested<129>>(other.as_bytes(), &canonical),
            Verdict::Wrong(_)
        ));
    }

    #[test]
    fn output_of_another_shape_is_wrong() {
        assert!(matches!(
            judge_encoding::<F32s>(b"{\"a\":1}", &F32s::bytes()),
            Verdict::Wrong(m) if m.starts_with("output does not decode as f32s")
        ));
    }

    #[test]
    fn output_that_is_not_json_is_wrong() {
        for bad in [
            &b"{\"a\":1} x"[..],
            b"{\"a\":1",
            b"[1,]",
            b"[01]",
            b"[1.]",
            b"[-]",
            b"[1e400]",
            b"\"\\ud800\"",
            b"\"\\x\"",
            b"\"\x01\"",
            b"\"\xff\"",
            b"{1:2}",
            b"tru",
            b"",
        ] {
            let verdict = compare_json(bad, b"{\"a\":1}");
            assert!(
                matches!(&verdict, Verdict::Wrong(m) if m.starts_with("output is not JSON")),
                "{}: {verdict:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
