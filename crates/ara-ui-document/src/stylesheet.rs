//! Restricted stylesheets with exact declaration spans, tokens and eager diagnostics.
use crate::{Diagnostic, Result, markup::TAGS};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range};

/// One source declaration, including the exact editable value range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Declaration {
    /// Property or token name.
    pub name: String,
    /// Unresolved value.
    pub value: String,
    /// Value bytes in the source stylesheet.
    pub bytes: Range<usize>,
}
/// A single type/class/ID selector with at most one state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    /// Original selector, without surrounding whitespace.
    pub selector: String,
    /// Ordered declarations; later duplicates win.
    pub declarations: Vec<Declaration>,
    /// Entire rule's byte range.
    pub bytes: Range<usize>,
    /// Position immediately before the closing brace.
    pub end: usize,
}
/// Validated syntax and global design tokens. Runtime themes override tokens explicitly.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stylesheet {
    /// Source-ordered rules, including token declarations.
    pub rules: Vec<Rule>,
    /// Global `:root` tokens.
    pub tokens: BTreeMap<String, String>,
}
/// Primitive selectors accepted by native stylesheets.
pub const TYPES: &[&str] = TAGS;
/// Properties accepted by native v1; unsupported browser CSS always fails.
pub const PROPERTIES: &[&str] = &[
    "display",
    "flex-direction",
    "flex-grow",
    "flex-shrink",
    "width",
    "height",
    "min-width",
    "min-height",
    "max-width",
    "max-height",
    "padding",
    "padding-left",
    "gap",
    "grid-template-columns",
    "background",
    "color",
    "font-size",
    "border-color",
    "border-width",
    "border-radius",
    "overflow",
    "translate-x",
    "translate-y",
    "align-items",
    "justify-content",
    "position",
    "left",
    "right",
    "top",
    "bottom",
];
fn error(source: &str, bytes: Range<usize>, message: impl Into<String>, fix: &str) -> Diagnostic {
    Diagnostic::new(7010, source, bytes, message, fix)
}
fn mask_comments(source: &str) -> Result<String> {
    let mut bytes = source.as_bytes().to_vec();
    let mut cursor = 0;
    while let Some(start) = source[cursor..].find("/*").map(|p| p + cursor) {
        let end = source[start + 2..]
            .find("*/")
            .map(|p| p + start + 4)
            .ok_or_else(|| {
                error(
                    source,
                    start..source.len(),
                    "Unclosed stylesheet comment",
                    "Close the comment with */",
                )
            })?;
        for b in &mut bytes[start..end] {
            if *b != b'\n' && *b != b'\r' {
                *b = b' ';
            }
        }
        cursor = end;
    }
    Ok(String::from_utf8(bytes).expect("comment masking only writes ASCII spaces"))
}
/// Parse an inline declaration block, retaining original offsets.
pub fn declarations(source: &str, offset: usize) -> Result<Vec<Declaration>> {
    let mut out = Vec::new();
    let mut cursor = 0;
    for raw in source.split(';') {
        if !raw.trim().is_empty() {
            let (name, value) = raw.split_once(':').ok_or_else(|| {
                error(
                    source,
                    cursor..cursor + raw.len(),
                    "Expected property: value",
                    "Separate the property and value with a colon",
                )
            })?;
            let name = name.trim();
            let value_offset = cursor + raw.find(':').unwrap_or_default() + 1;
            let start = value_offset + value.len() - value.trim_start().len();
            let value = value.trim();
            if !name.starts_with("--") && !PROPERTIES.contains(&name) {
                return Err(error(
                    source,
                    cursor..cursor + raw.len(),
                    format!("Unsupported property {name}"),
                    "Use a documented native CSS property",
                ));
            }
            if value.is_empty() || value.contains(['{', '}', '!', '"', '\'']) {
                return Err(error(
                    source,
                    start..start + value.len(),
                    "Unsupported declaration value",
                    "Use a literal native value or var(--token)",
                ));
            }
            out.push(Declaration {
                name: name.into(),
                value: value.into(),
                bytes: offset + start..offset + start + value.len(),
            });
        }
        cursor += raw.len() + 1;
    }
    Ok(out)
}
impl Stylesheet {
    /// Parse every rule and resolve even unused tokens to reject cycles early.
    pub fn parse(source: &str) -> Result<Self> {
        let clean = mask_comments(source)?;
        let mut sheet = Self::default();
        let mut cursor = 0;
        while !clean[cursor..].trim().is_empty() {
            let start = cursor + clean[cursor..].len() - clean[cursor..].trim_start().len();
            let open = clean[start..].find('{').map(|p| start + p).ok_or_else(|| {
                error(
                    source,
                    start..source.len(),
                    "Missing rule body",
                    "Add { declarations }",
                )
            })?;
            let end = clean[open + 1..]
                .find('}')
                .map(|p| open + 1 + p)
                .ok_or_else(|| {
                    error(
                        source,
                        open..source.len(),
                        "Unclosed rule",
                        "Close the rule with }",
                    )
                })?;
            let selector = clean[start..open].trim();
            if selector != ":root" {
                let (base, state) = selector
                    .split_once(':')
                    .map_or((selector, None), |(b, s)| (b, Some(s)));
                let name = base.trim_start_matches(['.', '#']);
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                    || (base == name && !TAGS.contains(&name))
                    || state
                        .is_some_and(|s| !matches!(s, "hover" | "focus" | "active" | "disabled"))
                {
                    return Err(error(
                        source,
                        start..open,
                        format!("Unsupported selector {selector}"),
                        "Use type, .class or #id with :hover, :focus, :active or :disabled",
                    ));
                }
            }
            let decl = declarations(&clean[open + 1..end], open + 1).map_err(|d| {
                Diagnostic::new(
                    7010,
                    source,
                    open + 1 + d.bytes.start..open + 1 + d.bytes.end,
                    d.message,
                    d.fix,
                )
            })?;
            for d in &decl {
                if selector == ":root" {
                    if !d.name.starts_with("--") {
                        return Err(error(
                            source,
                            d.bytes.clone(),
                            "Root accepts tokens only",
                            "Move this property into a widget rule",
                        ));
                    }
                    sheet.tokens.insert(d.name.clone(), d.value.clone());
                } else if d.name.starts_with("--") {
                    return Err(error(
                        source,
                        d.bytes.clone(),
                        "Scoped tokens are unsupported",
                        "Declare tokens in :root",
                    ));
                }
            }
            sheet.rules.push(Rule {
                selector: selector.into(),
                declarations: decl,
                bytes: start..end + 1,
                end,
            });
            cursor = end + 1;
        }
        for v in sheet.tokens.values() {
            sheet.resolve(v)?;
        }
        Ok(sheet)
    }
    /// Resolve a literal or whole-value token reference with cycle detection.
    pub fn resolve(&self, value: &str) -> Result<String> {
        let mut current = value;
        let mut seen = std::collections::BTreeSet::new();
        while let Some(name) = current
            .strip_prefix("var(")
            .and_then(|v| v.strip_suffix(')'))
        {
            if !seen.insert(name) {
                return Err(error(
                    "",
                    0..0,
                    format!("Cyclic token {name}"),
                    "Remove the token cycle",
                ));
            }
            current = self.tokens.get(name).ok_or_else(|| {
                error(
                    "",
                    0..0,
                    format!("Unknown token {name}"),
                    "Define the semantic token or select a built-in theme",
                )
            })?;
        }
        Ok(current.into())
    }
    /// Reject literal editor colors, including values in unused rules and token overrides.
    pub fn check_editor(&self, source: &str) -> Result<()> {
        for rule in &self.rules {
            for d in &rule.declarations {
                if d.value.contains('#') || d.value.contains("rgb(") || d.value.contains("rgba(") {
                    return Err(Diagnostic::new(
                        7018,
                        source,
                        d.bytes.clone(),
                        "Raw colors are forbidden in editor styles",
                        "Use var(--background), var(--foreground), var(--border) or another semantic theme token",
                    ));
                }
            }
        }
        Ok(())
    }
}
