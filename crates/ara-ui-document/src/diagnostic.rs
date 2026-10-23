use serde::{Deserialize, Serialize};
use std::{fmt, ops::Range};

/// Located, stable UI diagnostic; `fix` explains the supported replacement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable code in the UI-owned ARA7000–ARA7099 range.
    pub code: String,
    /// Human-readable reason the source or operation was rejected.
    pub message: String,
    /// UTF-8 byte range in the original source.
    pub bytes: Range<usize>,
    /// One-based source line.
    pub line: usize,
    /// One-based Unicode column.
    pub column: usize,
    /// Actionable supported syntax or recovery advice.
    pub fix: String,
}
impl Diagnostic {
    /// Construct a diagnostic with source coordinates, clamping to UTF-8 boundaries.
    pub fn new(
        code: u16,
        source: &str,
        bytes: Range<usize>,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        let mut start = bytes.start.min(source.len());
        while !source.is_char_boundary(start) {
            start -= 1;
        }
        let prefix = &source[..start];
        Self {
            code: format!("ARA{code}"),
            message: message.into(),
            bytes: start..bytes.end.min(source.len()).max(start),
            line: prefix.bytes().filter(|b| *b == b'\n').count() + 1,
            column: prefix
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .count()
                + 1,
            fix: fix.into(),
        }
    }
}
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {}:{}: {}. Fix: {}",
            self.code, self.line, self.column, self.message, self.fix
        )
    }
}
impl std::error::Error for Diagnostic {}
/// Fallible source compilation or editing result.
pub type Result<T> = std::result::Result<T, Diagnostic>;
