//! Tokens for the `.audiograph` text format.

use crate::codes;
use ara_diag::Diagnostic;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Tok {
    /// Identifier or dotted path (`lowpass`, `noise.white`, `player.done`).
    Word(String),
    Number(f32),
    /// `@path` or `@path#id`.
    Clip(String),
    Str(String),
    Punct(char),
}

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub tok: Tok,
    pub offset: usize,
}

/// 1-based line and column of a byte offset, for diagnostics.
pub(super) fn position(source: &str, offset: usize) -> (u32, u32) {
    let before = &source[..offset.min(source.len())];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before.len(), |n| before.len() - n - 1)
        + 1;
    (line as u32, column as u32)
}

pub(super) fn error(source: &str, offset: usize, message: impl Into<String>) -> Diagnostic {
    let (line, column) = position(source, offset);
    Diagnostic::error(codes::GRAPH, message).line_col(line, column)
}

fn word_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

pub(super) fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if source[i..].starts_with("//") {
            i = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
            continue;
        }
        let start = i;
        let tok = if b == b'"' {
            i += 1;
            let mut escaped = false;
            loop {
                let Some(&c) = bytes.get(i) else {
                    return Err(error(source, start, "unterminated string"));
                };
                i += 1;
                if c == b'"' && !escaped {
                    break;
                }
                escaped = c == b'\\' && !escaped;
            }
            let value =
                ara_json::parse(&source[start..i]).map_err(|e| error(source, start, e.message))?;
            Tok::Str(value.as_str().unwrap_or_default().to_owned())
        } else if b == b'@' {
            i += 1;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !b",;(){}".contains(&bytes[i])
            {
                i += 1;
            }
            Tok::Clip(source[start..i].to_owned())
        } else if b.is_ascii_digit()
            || (b == b'-' || b == b'.')
                && bytes
                    .get(i + 1)
                    .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
        {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || bytes[i] == b'.'
                    || bytes[i] == b'e'
                    || bytes[i] == b'E'
                    || (matches!(bytes[i], b'-' | b'+') && matches!(bytes[i - 1], b'e' | b'E')))
            {
                i += 1;
            }
            let text = &source[start..i];
            let value: f32 = text
                .parse()
                .map_err(|_| error(source, start, format!("invalid number `{text}`")))?;
            if !value.is_finite() {
                return Err(error(
                    source,
                    start,
                    format!("number `{text}` is not finite"),
                ));
            }
            Tok::Number(value)
        } else if b.is_ascii_alphabetic() || b == b'_' {
            while i < bytes.len()
                && (word_char(bytes[i])
                    || bytes[i] == b'.'
                        && bytes.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic()))
            {
                i += 1;
            }
            Tok::Word(source[start..i].to_owned())
        } else if b"{}(),;:=".contains(&b) {
            i += 1;
            Tok::Punct(b as char)
        } else {
            return Err(error(
                source,
                start,
                format!(
                    "unexpected character `{}`",
                    source[i..].chars().next().unwrap_or('?')
                ),
            ));
        };
        tokens.push(Token { tok, offset: start });
    }
    Ok(tokens)
}
