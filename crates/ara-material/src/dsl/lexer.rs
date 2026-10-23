use crate::codes;
use ara_diag::Diagnostic;

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub text: String,
    pub offset: usize,
    pub quoted: bool,
}

pub(super) fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let b = source.as_bytes()[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if source[i..].starts_with("//") {
            i = source[i..].find('\n').map_or(source.len(), |n| i + n);
            continue;
        }
        let start = i;
        if b == b'"' {
            i += 1;
            let mut escaped = false;
            let mut closed = false;
            while i < source.len() {
                let c = source.as_bytes()[i];
                i += 1;
                if c == b'"' && !escaped {
                    closed = true;
                    break;
                }
                escaped = c == b'\\' && !escaped;
            }
            if !closed {
                return Err(Diagnostic::error(codes::SYNTAX, "unterminated string"));
            }
            let value = ara_json::parse(&source[start..i])
                .map_err(|e| Diagnostic::error(codes::SYNTAX, e.message))?;
            tokens.push(Token {
                text: value.as_str().unwrap_or("").into(),
                offset: start,
                quoted: true,
            });
            continue;
        }
        if b == b'@' {
            i += 1;
            while i < source.len()
                && !source.as_bytes()[i].is_ascii_whitespace()
                && !b",;(){}[]".contains(&source.as_bytes()[i])
            {
                i += 1;
            }
        } else if b == b'#' {
            i += 1;
            while i < source.len()
                && (source.as_bytes()[i].is_ascii_alphanumeric() || source.as_bytes()[i] == b'_')
            {
                i += 1;
            }
        } else if source[i..].starts_with("->") {
            i += 2;
        } else if b.is_ascii_alphabetic() || b == b'_' {
            i += 1;
            while i < source.len()
                && (source.as_bytes()[i].is_ascii_alphanumeric()
                    || b"_.".contains(&source.as_bytes()[i]))
            {
                i += 1;
            }
        } else if b.is_ascii_digit() || b == b'-' || b == b'+' || b == b'.' {
            i += 1;
            while i < source.len()
                && (source.as_bytes()[i].is_ascii_digit()
                    || b".eE+-".contains(&source.as_bytes()[i]))
            {
                i += 1;
            }
        } else if b"{}():=;,[]".contains(&b) {
            i += 1;
        } else {
            return Err(Diagnostic::error(
                codes::SYNTAX,
                format!("unexpected character at byte {i}"),
            ));
        }
        tokens.push(Token {
            text: source[start..i].into(),
            offset: start,
            quoted: false,
        });
    }
    Ok(tokens)
}
