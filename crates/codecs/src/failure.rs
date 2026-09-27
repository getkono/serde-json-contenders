//! Why a decode or encode failed, in the terms a server answers with.

/// serde_json's error categories, which a server maps to a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Not valid JSON.
    Syntax,
    /// Valid so far, then the input ended.
    Eof,
    /// Valid JSON of the wrong shape for the target type.
    Data,
    /// An I/O failure.
    Io,
    /// The backend has no such operation.
    Unsupported,
}

/// Where a backend says the failure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// One-based line and column.
    LineColumn(usize, usize),
    /// Zero-based byte offset.
    Offset(usize),
}

/// A failure, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The category.
    pub class: Class,
    /// Where, if the backend says.
    pub position: Option<Position>,
    /// The backend's message.
    pub message: String,
}

impl Failure {
    /// A failure of `class` with no position.
    #[must_use]
    pub fn new(class: Class, message: impl std::fmt::Display) -> Self {
        Self {
            class,
            position: None,
            message: message.to_string(),
        }
    }

    /// Attach a position.
    #[must_use]
    pub fn at(mut self, position: Position) -> Self {
        self.position = Some(position);
        self
    }

    /// The operation does not exist in this backend.
    #[must_use]
    pub fn unsupported() -> Self {
        Self::new(Class::Unsupported, "unsupported by this backend")
    }

    /// The status a server answers with: 422 for a well-formed body of the
    /// wrong shape, 400 for everything else.
    #[must_use]
    pub fn status(&self) -> u16 {
        if self.class == Class::Data { 422 } else { 400 }
    }

    /// Resolve the position to one-based line and column against `input`.
    #[must_use]
    pub fn line_column(&self, input: &[u8]) -> Option<(usize, usize)> {
        match self.position? {
            Position::LineColumn(line, column) => Some((line, column)),
            Position::Offset(offset) => {
                let before = &input[..offset.min(input.len())];
                let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
                let column = before.iter().rev().take_while(|&&b| b != b'\n').count() + 1;
                Some((line, column))
            }
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.class, self.message)
    }
}
