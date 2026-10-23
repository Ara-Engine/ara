//! Tokens with line/column positions. `#` and `//` start line comments.
use crate::codes;
use ara_diag::Diagnostic;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Ident(String),
    Num(f64),
    Str(String),
    Punct(&'static str),
    End,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub tok: Tok,
    pub line: u32,
    pub col: u32,
}

const PUNCT: [&str; 22] = [
    "->", "==", "!=", "<=", ">=", "{", "}", "(", ")", "[", "]", ";", ":", ",", "=", "<", ">", "+",
    "-", "*", "/", ".",
];

pub(crate) fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let mut out = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let (mut i, mut line, mut col) = (0usize, 1u32, 1u32);
    let error = |line: u32, col: u32, msg: String| {
        Diagnostic::error(codes::SYNTAX, msg).line_col(line, col)
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        if c == '#' || (c == '/' && chars.get(i + 1) == Some(&'/')) {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let (start_line, start_col) = (line, col);
        if c.is_ascii_alphabetic() || c == '_' {
            let s: String = chars[i..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                .collect();
            i += s.len();
            col += s.len() as u32;
            out.push(Token {
                tok: Tok::Ident(s),
                line: start_line,
                col: start_col,
            });
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let mut s = String::new();
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || ((chars[i] == 'e' || chars[i] == 'E')
                        && chars
                            .get(i + 1)
                            .is_some_and(|n| n.is_ascii_digit() || *n == '-')))
            {
                s.push(chars[i]);
                if (chars[i] == 'e' || chars[i] == 'E') && chars.get(i + 1) == Some(&'-') {
                    s.push('-');
                    i += 1;
                    col += 1;
                }
                i += 1;
                col += 1;
            }
            let value: f64 = s
                .parse()
                .map_err(|_| error(start_line, start_col, format!("invalid number `{s}`")))?;
            if !value.is_finite() {
                return Err(error(
                    start_line,
                    start_col,
                    format!("number `{s}` is not finite"),
                ));
            }
            out.push(Token {
                tok: Tok::Num(value),
                line: start_line,
                col: start_col,
            });
            continue;
        }
        if c == '"' {
            let mut s = String::new();
            i += 1;
            col += 1;
            loop {
                match chars.get(i) {
                    None | Some('\n') => {
                        return Err(error(start_line, start_col, "unterminated string".into()));
                    }
                    Some('"') => {
                        i += 1;
                        col += 1;
                        break;
                    }
                    Some('\\') if chars.get(i + 1) == Some(&'"') => {
                        s.push('"');
                        i += 2;
                        col += 2;
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                        col += 1;
                    }
                }
            }
            out.push(Token {
                tok: Tok::Str(s),
                line: start_line,
                col: start_col,
            });
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
        let Some(p) = PUNCT.iter().find(|p| rest.starts_with(**p)) else {
            return Err(error(line, col, format!("unexpected character `{c}`")));
        };
        i += p.len();
        col += p.len() as u32;
        out.push(Token {
            tok: Tok::Punct(p),
            line: start_line,
            col: start_col,
        });
    }
    out.push(Token {
        tok: Tok::End,
        line,
        col,
    });
    Ok(out)
}
