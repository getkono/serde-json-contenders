//! An exact JSON comparator that trusts no backend, serde_json included.
//!
//! A minimal strict RFC 8259 parser to a tree whose numbers keep their source
//! text. Two documents are semantically equal when their trees match: strings
//! after unescaping, object members in order, and numbers by value —
//! integers exactly, anything else by the bits `str::parse::<f64>` (correctly
//! rounded) gives. So `1.50` equals `1.5`, and `-0.0` never equals `0.0`.

/// A parsed document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as written.
    Number(String),
    /// A string, unescaped.
    String(String),
    /// An array.
    Array(Vec<Node>),
    /// An object, members in document order.
    Object(Vec<(String, Node)>),
}

/// Parse exactly one JSON document, surrounding whitespace allowed.
pub fn parse(input: &[u8]) -> Result<Node, String> {
    let text = std::str::from_utf8(input).map_err(|e| format!("invalid UTF-8: {e}"))?;
    let mut parser = Parser {
        text: text.as_bytes(),
        at: 0,
    };
    parser.whitespace();
    let node = parser.value()?;
    parser.whitespace();
    if parser.at != parser.text.len() {
        return Err(format!("trailing characters at byte {}", parser.at));
    }
    Ok(node)
}

/// Whether two numbers, as JSON text, denote the same value.
#[must_use]
pub fn numbers_equal(a: &str, b: &str) -> bool {
    let integer = |s: &str| !s.contains(['.', 'e', 'E']);
    if integer(a) && integer(b) {
        return match (a.parse::<i128>(), b.parse::<i128>()) {
            (Ok(x), Ok(y)) => x == y,
            _ => normalize_integer(a) == normalize_integer(b),
        };
    }
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.to_bits() == y.to_bits(),
        _ => false,
    }
}

/// An integer beyond `i128`, without leading zeros or a negative zero.
fn normalize_integer(s: &str) -> String {
    let (sign, digits) = s.strip_prefix('-').map_or(("", s), |d| ("-", d));
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        "0".to_owned()
    } else {
        format!("{sign}{digits}")
    }
}

/// Whether two trees are semantically equal.
#[must_use]
pub fn same(a: &Node, b: &Node) -> bool {
    match (a, b) {
        (Node::Number(x), Node::Number(y)) => numbers_equal(x, y),
        (Node::Array(x), Node::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (Node::Object(x), Node::Object(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|((kx, x), (ky, y))| kx == ky && same(x, y))
        }
        _ => a == b,
    }
}

/// Whether a decoded `Value` holds exactly what `node` says: object members
/// by key (a `Value` map has its own order), numbers as in [`numbers_equal`].
/// A float decoded correctly rounded matches its source text.
#[must_use]
pub fn matches_value(node: &Node, value: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (node, value) {
        (Node::Null, Value::Null) => true,
        (Node::Bool(x), Value::Bool(y)) => x == y,
        (Node::String(x), Value::String(y)) => x == y,
        (Node::Number(x), Value::Number(y)) => numbers_equal(x, &y.to_string()),
        (Node::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| matches_value(x, y)),
        (Node::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(key, x)| y.get(key).is_some_and(|y| matches_value(x, y)))
        }
        _ => false,
    }
}

/// Whether two documents are semantically equal; an error names the side
/// that is not JSON.
pub fn semantically_equal(reference: &[u8], backend: &[u8]) -> Result<bool, String> {
    let reference = parse(reference).map_err(|e| format!("reference is not JSON: {e}"))?;
    let backend = parse(backend).map_err(|e| format!("backend output is not JSON: {e}"))?;
    Ok(same(&reference, &backend))
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn error<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.at))
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn literal(&mut self, word: &str, node: Node) -> Result<Node, String> {
        if self.text[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(node)
        } else {
            self.error("invalid literal")
        }
    }

    fn value(&mut self) -> Result<Node, String> {
        match self.peek() {
            Some(b'n') => self.literal("null", Node::Null),
            Some(b't') => self.literal("true", Node::Bool(true)),
            Some(b'f') => self.literal("false", Node::Bool(false)),
            Some(b'"') => self.string().map(Node::String),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => self.error("expected a value"),
            None => self.error("unexpected end"),
        }
    }

    fn array(&mut self) -> Result<Node, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Node::Array(items));
        }
        loop {
            self.whitespace();
            items.push(self.value()?);
            self.whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Node::Array(items));
                }
                _ => return self.error("expected , or ]"),
            }
        }
    }

    fn object(&mut self) -> Result<Node, String> {
        self.at += 1;
        let mut members = Vec::new();
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Node::Object(members));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return self.error("expected a key");
            }
            let key = self.string()?;
            self.whitespace();
            if self.peek() != Some(b':') {
                return self.error("expected :");
            }
            self.at += 1;
            self.whitespace();
            members.push((key, self.value()?));
            self.whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Node::Object(members));
                }
                _ => return self.error("expected , or }"),
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<Node, String> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return self.error("expected a digit"),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return self.error("expected a fraction digit");
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return self.error("expected an exponent digit");
            }
        }
        let text = std::str::from_utf8(&self.text[start..self.at]).map_err(|e| e.to_string())?;
        Ok(Node::Number(text.to_owned()))
    }

    fn hex4(&mut self) -> Result<u16, String> {
        let Some(digits) = self.text.get(self.at..self.at + 4) else {
            return self.error("truncated \\u escape");
        };
        let digits = std::str::from_utf8(digits).map_err(|e| e.to_string())?;
        let unit = u16::from_str_radix(digits, 16).map_err(|_| format!("invalid \\u escape at byte {}", self.at))?;
        self.at += 4;
        Ok(unit)
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let start = self.at;
            while matches!(self.peek(), Some(c) if c != b'"' && c != b'\\' && c >= 0x20) {
                self.at += 1;
            }
            // Input is valid UTF-8 and the run stops only at ASCII.
            out.push_str(std::str::from_utf8(&self.text[start..self.at]).map_err(|e| e.to_string())?);
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escape = self.peek();
                    self.at += 1;
                    match escape {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => out.push(self.unicode_escape()?),
                        _ => return self.error("invalid escape"),
                    }
                }
                Some(_) => return self.error("unescaped control character"),
                None => return self.error("unterminated string"),
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let unit = self.hex4()?;
        let code = match unit {
            0xD800..=0xDBFF => {
                if !self.text[self.at..].starts_with(b"\\u") {
                    return self.error("lone high surrogate");
                }
                self.at += 2;
                let low = self.hex4()?;
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return self.error("high surrogate not followed by a low one");
                }
                0x10000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00)
            }
            0xDC00..=0xDFFF => return self.error("lone low surrogate"),
            _ => u32::from(unit),
        };
        char::from_u32(code).map_or_else(|| self.error("invalid code point"), Ok)
    }
}

#[cfg(test)]
mod tests;
