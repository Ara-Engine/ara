use super::put;
use crate::{Result, Status, StringEntry, Table, error};
use ara_cmd::{Command, EditCtx};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Source language scanned for localization candidates.
#[derive(Debug, Clone, Copy, Schema, Serialize, Deserialize)]
#[ara(name = "L10nSourceKind", module = "l10n")]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    /// UI text nodes and human-visible text/label/placeholder/title attributes.
    Markup,
    /// C# normal and verbatim string literals, excluding comments and char literals.
    Csharp,
}
/// Source-scan request. Sources are supplied by the host; no arbitrary filesystem IO.
#[derive(Schema, Deserialize)]
#[ara(name = "L10nExtractParams", params, module = "l10n")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtractParams {
    /// UI markup or C#.
    pub kind: SourceKind,
    /// Complete UTF-8 source text.
    pub source: String,
    /// Dotted prefix for generated stable keys, for example `pause`.
    pub prefix: String,
}
/// A reviewable source replacement plus the corresponding table key.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extraction {
    /// Decoded human-visible literal.
    pub literal: String,
    /// Stable generated or reused lookup key.
    pub key: String,
    /// Inclusive UTF-8 byte offset of the source token/content.
    pub start: usize,
    /// Exclusive UTF-8 byte offset; apply replacements in reverse order.
    pub end: usize,
    /// Suggested source replacement; host reviews/applies code edits separately.
    pub replacement: String,
}
/// Scans literals without changing code. Interpolated/raw C# strings are diagnosed
/// rather than converted to constant text with lost runtime parameters.
pub fn extract(kind: SourceKind, source: &str, prefix: &str) -> Result<Vec<Extraction>> {
    crate::table::validate_key(prefix)?;
    if source.len() > 16 * 1024 * 1024 {
        return Err(error("extraction source exceeds 16 MiB"));
    }
    let candidates = match kind {
        SourceKind::Markup => markup(source)?,
        SourceKind::Csharp => csharp(source)?,
    };
    Ok(candidates
        .into_iter()
        .filter(|(_, _, literal)| {
            !literal.trim().is_empty()
                && !literal.starts_with("loc:")
                && !literal.trim_start().starts_with('{')
        })
        .map(|(start, end, literal)| {
            let key = format!(
                "{prefix}.text_{}",
                &blake3::hash(literal.as_bytes()).to_hex()[..12]
            );
            let replacement = match kind {
                SourceKind::Markup => format!("loc:{key}"),
                SourceKind::Csharp => format!("Loc.Text(\"{key}\")"),
            };
            Extraction {
                literal,
                key,
                start,
                end,
                replacement,
            }
        })
        .collect())
}
type Candidate = (usize, usize, String);
fn markup(source: &str) -> Result<Vec<Candidate>> {
    // roxmltree supplies exact original attribute/text ranges and decodes entities.
    let document = roxmltree::Document::parse(source).map_err(|e| error(e.to_string()))?;
    let mut output = Vec::new();
    for node in document.descendants() {
        if node.is_text()
            && node
                .parent_element()
                .is_none_or(|p| !matches!(p.tag_name().name(), "style" | "script"))
        {
            let range = node.range();
            let literal = node.text().unwrap_or_default();
            let leading = source[range.clone()].len() - source[range.clone()].trim_start().len();
            let trailing = source[range.clone()].len() - source[range.clone()].trim_end().len();
            if range.start + leading < range.end - trailing {
                output.push((
                    range.start + leading,
                    range.end - trailing,
                    literal.trim().into(),
                ));
            }
        }
        for attribute in node.attributes() {
            if matches!(
                attribute.name(),
                "text" | "label" | "placeholder" | "title" | "aria-label"
            ) {
                let range = attribute.range_value();
                output.push((range.start, range.end, attribute.value().into()));
            }
        }
    }
    Ok(output)
}
fn csharp(source: &str) -> Result<Vec<Candidate>> {
    let bytes = source.as_bytes();
    let mut at = 0;
    let mut output = Vec::new();
    while at < bytes.len() {
        if bytes[at..].starts_with(b"//") {
            at = source[at..].find('\n').map_or(bytes.len(), |n| at + n);
        } else if bytes[at..].starts_with(b"/*") {
            at = source[at + 2..]
                .find("*/")
                .map(|n| at + n + 4)
                .ok_or_else(|| error("unterminated C# comment"))?;
        } else if bytes[at] == b'\'' {
            at += 1;
            while at < bytes.len() {
                if bytes[at] == b'\\' {
                    at += 2;
                } else if bytes[at] == b'\'' {
                    at += 1;
                    break;
                } else {
                    at += 1;
                }
            }
        } else if bytes[at..].starts_with(b"$\"")
            || bytes[at..].starts_with(b"$@\"")
            || bytes[at..].starts_with(b"@$\"")
            || bytes[at..].starts_with(b"\"\"\"")
        {
            return Err(error(format!(
                "C# interpolated/raw string at byte {at} requires an explicit key and typed parameters"
            )));
        } else if bytes[at] == b'"' || bytes[at..].starts_with(b"@\"") {
            let start = at;
            let verbatim = bytes[at] == b'@';
            at += if verbatim { 2 } else { 1 };
            let content = at;
            let mut value = String::new();
            let mut closed = false;
            while at < bytes.len() {
                if bytes[at] == b'"' {
                    if verbatim && bytes.get(at + 1) == Some(&b'"') {
                        value.push('"');
                        at += 2;
                    } else {
                        at += 1;
                        closed = true;
                        break;
                    }
                } else if !verbatim && bytes[at] == b'\\' {
                    let escape_start = at;
                    at += 1;
                    let escape = *bytes.get(at).ok_or_else(|| error("truncated C# escape"))?;
                    at += 1;
                    match escape {
                        b'n' => value.push('\n'),
                        b'r' => value.push('\r'),
                        b't' => value.push('\t'),
                        b'0' => value.push('\0'),
                        b'a' => value.push('\u{7}'),
                        b'b' => value.push('\u{8}'),
                        b'f' => value.push('\u{c}'),
                        b'v' => value.push('\u{b}'),
                        b'\\' => value.push('\\'),
                        b'"' => value.push('"'),
                        b'\'' => value.push('\''),
                        b'u' | b'U' | b'x' => {
                            let length = match escape {
                                b'u' => 4,
                                b'U' => 8,
                                _ => bytes[at..]
                                    .iter()
                                    .take(4)
                                    .take_while(|b| b.is_ascii_hexdigit())
                                    .count(),
                            };
                            let digits = source
                                .get(at..at + length)
                                .ok_or_else(|| error("truncated C# Unicode escape"))?;
                            let code = u32::from_str_radix(digits, 16)
                                .map_err(|_| error("invalid C# Unicode escape"))?;
                            value.push(char::from_u32(code).ok_or_else(|| {
                                error("C# surrogate escapes require scalar Unicode input")
                            })?);
                            at += length;
                        }
                        _ => return Err(error(format!("unsupported C# escape at {escape_start}"))),
                    }
                } else {
                    let c = source[at..]
                        .chars()
                        .next()
                        .ok_or_else(|| error("invalid C# string"))?;
                    value.push(c);
                    at += c.len_utf8();
                }
            }
            if !closed {
                return Err(error(format!("unterminated C# string at {content}")));
            }
            output.push((start, at, value));
        } else {
            at += 1;
        }
    }
    Ok(output)
}
/// Extracts literals into the current table, returning reviewable source patches.
pub struct Extract;
impl Command for Extract {
    type Params = ExtractParams;
    type Output = Vec<Extraction>;
    const NAME: &'static str = "l10n.extract";
    const SUMMARY: &'static str =
        "Scan UI/C# literals into reviewed key proposals and undoable table ops.";
    const SCOPE: &'static str = "edit:l10n";
    fn examples() -> Vec<Value> {
        vec![json!({"kind":"markup","source":"<ui><label text='Resume'/></ui>","prefix":"pause"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Self::Output> {
        let mut extractions = extract(params.kind, &params.source, &params.prefix)?;
        let table = Table::from_document(ctx.read().doc().clone())?;
        for extraction in &mut extractions {
            let pattern = extraction
                .literal
                .replace('\'', "''")
                .replace('{', "'{'")
                .replace('}', "'}'");
            if let Some(existing) = table
                .entries()
                .find(|e| e.text == pattern && e.params.is_empty())
            {
                extraction.replacement = extraction
                    .replacement
                    .replace(&extraction.key, &existing.key);
                extraction.key = existing.key.clone();
            } else {
                put(
                    ctx,
                    None,
                    StringEntry {
                        key: extraction.key.clone(),
                        text: pattern,
                        status: Status::NeedsReview,
                        ..Default::default()
                    },
                )?;
            }
        }
        Ok(extractions)
    }
}
