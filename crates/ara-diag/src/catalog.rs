//! Every diagnostic code's documentation: title, explanation, common causes and
//! fixes. The source is markdown in `crates/ara-diag/codes/*.md` (one file per code
//! family, like rustc's error index), embedded at build time and parsed once.
//!
//! ```text
//! ## ARA1302 UNRESOLVED_REF: Unresolved reference
//! owner: ara-doc
//!
//! What it means, in one or more paragraphs.
//!
//! Causes:
//! - One cause per line.
//!
//! Fixes:
//! - One fix per line.
//! ```
//!
//! Optional meta lines after `owner:`: `shown as: ARA-EXP-AND-007` (a *named* code:
//! it keeps the name it was born with; the number is an internal slot in
//! `6000`–`6999` or `8000`–`9999`) and `aliases: A, B` (other names that look it
//! up). `ara why <code>`, `ara_docs` and the generated `errors/<code>` pages all
//! read this catalog; a test checks every code in the sources has an entry.

use std::sync::OnceLock;

use crate::Code;

/// The catalog sources, in display order.
const SOURCES: [(&str, &str); 8] = [
    ("csharp.md", include_str!("../codes/csharp.md")),
    ("kernel.md", include_str!("../codes/kernel.md")),
    ("bus.md", include_str!("../codes/bus.md")),
    ("runtime.md", include_str!("../codes/runtime.md")),
    ("subsystems.md", include_str!("../codes/subsystems.md")),
    ("gpu.md", include_str!("../codes/gpu.md")),
    ("export.md", include_str!("../codes/export.md")),
    ("collab.md", include_str!("../codes/collab.md")),
];

/// One code's documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeDoc {
    /// The code.
    pub code: Code,
    /// Constant name (`UNRESOLVED_REF`; `material::SYNTAX` outside the kernel).
    pub name: &'static str,
    /// For named codes, the name shown instead of `ARA####` (`ARA-EXP-AND-007`).
    pub label: Option<&'static str>,
    /// Other names that look the code up.
    pub aliases: Vec<&'static str>,
    /// Crates or components that report it.
    pub owner: &'static str,
    /// A few words (`Unresolved reference`).
    pub title: &'static str,
    /// What it means (markdown paragraphs).
    pub explanation: &'static str,
    /// Common causes, most likely first.
    pub causes: Vec<&'static str>,
    /// Fixes, best first.
    pub fixes: Vec<&'static str>,
}

impl CodeDoc {
    /// The explanation's first sentence (or line), for one-line listings.
    pub fn summary(&self) -> &'static str {
        let text = self.explanation;
        let end = text
            .find(". ")
            .map(|i| i + 1)
            .or_else(|| text.find('\n'))
            .unwrap_or(text.len());
        &text[..end]
    }

    /// The docs page body (markdown, without the title).
    pub fn markdown(&self) -> String {
        let mut s = format!("{}\n\n", self.explanation);
        s.push_str("## Common causes\n\n");
        for c in &self.causes {
            s.push_str(&format!("- {c}\n"));
        }
        s.push_str("\n## Fixes\n\n");
        for f in &self.fixes {
            s.push_str(&format!("- {f}\n"));
        }
        s.push_str(&format!("\n- Code: `{}`", self.code));
        if self.label.is_some() {
            s.push_str(&format!(" (slot {})", self.code.0));
        }
        s.push_str(&format!(
            "\n- Constant: `{}`\n- Reported by: {}\n",
            self.name, self.owner
        ));
        if !self.aliases.is_empty() {
            s.push_str(&format!("- Also known as: {}\n", self.aliases.join(", ")));
        }
        s
    }
}

/// The parsed catalog.
struct Catalog {
    /// Sorted by code.
    docs: Vec<CodeDoc>,
    /// Parse problems (a test asserts there are none).
    problems: Vec<String>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut docs = Vec::new();
        let mut problems = Vec::new();
        for (file, text) in SOURCES {
            parse(file, text, &mut docs, &mut problems);
        }
        docs.sort_by_key(|d| d.code);
        Catalog { docs, problems }
    })
}

/// Parses one catalog file.
fn parse(file: &str, text: &'static str, docs: &mut Vec<CodeDoc>, problems: &mut Vec<String>) {
    if text.starts_with("## ") {
        problems.push(format!("{file}: start with a `#` title, not an entry"));
    }
    for section in text.split("\n## ").skip(1) {
        let section = section.trim_end();
        match entry(section) {
            Ok(doc) => docs.push(doc),
            Err(e) => {
                let head = section.lines().next().unwrap_or_default();
                problems.push(format!("{file}: `## {head}`: {e}"));
            }
        }
    }
}

/// Parses one `## ARA#### NAME: Title` section.
fn entry(section: &'static str) -> Result<CodeDoc, String> {
    let (head, body) = section.split_once('\n').unwrap_or((section, ""));
    let (code_name, title) = head
        .split_once(": ")
        .ok_or("the heading needs `ARA#### NAME: Title`")?;
    let (code, name) = code_name
        .split_once(' ')
        .ok_or("the heading needs a constant name after the code")?;
    let number: u16 = code
        .strip_prefix("ARA")
        .filter(|n| n.len() == 4)
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("`{code}` is not `ARA` plus four digits"))?;
    let mut doc = CodeDoc {
        code: Code(number),
        name: name.trim(),
        label: None,
        aliases: Vec::new(),
        owner: "",
        title: title.trim(),
        explanation: "",
        causes: Vec::new(),
        fixes: Vec::new(),
    };
    let mut rest = body.trim_start_matches('\n');
    while let Some((line, after)) = rest.split_once('\n') {
        let line = line.trim();
        if let Some(owner) = line.strip_prefix("owner:") {
            doc.owner = owner.trim();
        } else if let Some(label) = line.strip_prefix("shown as:") {
            doc.label = Some(label.trim());
        } else if let Some(aliases) = line.strip_prefix("aliases:") {
            doc.aliases = aliases
                .split(',')
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .collect();
        } else {
            break;
        }
        rest = after;
    }
    if doc.owner.is_empty() {
        return Err("missing `owner:`".into());
    }
    let causes_at = rest.find("\nCauses:\n").ok_or("missing a `Causes:` list")?;
    doc.explanation = rest[..causes_at].trim();
    let after_causes = &rest[causes_at + "\nCauses:\n".len()..];
    let fixes_at = after_causes
        .find("\nFixes:\n")
        .ok_or("missing a `Fixes:` list after the causes")?;
    doc.causes = items(&after_causes[..fixes_at])?;
    doc.fixes = items(&after_causes[fixes_at + "\nFixes:\n".len()..])?;
    if doc.explanation.is_empty() {
        return Err("missing the explanation".into());
    }
    if doc.title.is_empty() || doc.name.contains(' ') {
        return Err("the heading needs one constant name and a title".into());
    }
    Ok(doc)
}

/// `- item` lines (one line each, at least one).
fn items(block: &'static str) -> Result<Vec<&'static str>, String> {
    let mut out = Vec::new();
    for line in block.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let item = line
            .strip_prefix("- ")
            .ok_or_else(|| format!("list items start with `- ` and fit on one line: `{line}`"))?;
        out.push(item.trim());
    }
    if out.is_empty() {
        return Err("a list is empty".into());
    }
    Ok(out)
}

/// Every documented code, sorted by number.
pub fn all() -> &'static [CodeDoc] {
    &catalog().docs
}

/// Problems found while parsing the catalog files (empty when they are well-formed).
pub fn problems() -> &'static [String] {
    &catalog().problems
}

/// The documentation of `code`.
pub fn get(code: Code) -> Option<&'static CodeDoc> {
    let docs = &catalog().docs;
    docs.binary_search_by_key(&code, |d| d.code)
        .ok()
        .map(|i| &docs[i])
}

/// True for the slots of named codes (`6000`–`6999`, `8000`–`9999`).
pub fn is_named_slot(code: Code) -> bool {
    matches!(code.0, 6000..=6999 | 8000..=9999)
}

/// The name a named code is shown as (`ARA-EXP-AND-007`).
pub fn label(code: Code) -> Option<&'static str> {
    if !is_named_slot(code) {
        return None;
    }
    get(code).and_then(|d| d.label)
}

/// Finds a code by any of its names: `ARA1302`, `ara1302`, `1302`, a label
/// (`ARA-EXP-AND-007`), an alias, or the constant name (`UNRESOLVED_REF`, or the
/// part after `::`). Case-insensitive.
pub fn lookup(text: &str) -> Option<&'static CodeDoc> {
    let t = text.trim().trim_matches('`');
    let digits = t
        .get(..3)
        .filter(|p| p.eq_ignore_ascii_case("ara"))
        .map_or(t, |_| &t[3..]);
    if digits.len() == 4
        && let Ok(n) = digits.parse::<u16>()
    {
        return get(Code(n));
    }
    let docs = all();
    let same = |a: &str| a.eq_ignore_ascii_case(t);
    docs.iter()
        .find(|d| d.label.is_some_and(same) || d.aliases.iter().any(|a| same(a)))
        .or_else(|| docs.iter().find(|d| same(d.name)))
        .or_else(|| {
            docs.iter()
                .find(|d| d.name.rsplit("::").next().is_some_and(same))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes;

    #[test]
    fn the_catalog_parses_cleanly_without_duplicates() {
        assert!(
            problems().is_empty(),
            "catalog problems:\n{}",
            problems().join("\n")
        );
        let docs = all();
        for pair in docs.windows(2) {
            assert_ne!(
                pair[0].code, pair[1].code,
                "{} is documented twice",
                pair[0].code
            );
        }
        let mut names: Vec<String> = docs
            .iter()
            .flat_map(|d| {
                let mut n = vec![d.name.to_lowercase()];
                n.extend(d.label.map(str::to_lowercase));
                n.extend(d.aliases.iter().map(|a| a.to_lowercase()));
                n
            })
            .collect();
        names.sort();
        for pair in names.windows(2) {
            assert_ne!(pair[0], pair[1], "two codes share the name `{}`", pair[0]);
        }
        for d in docs {
            assert_eq!(
                d.label.is_some(),
                is_named_slot(d.code),
                "{}: named codes (and only they) use the 6000/8000-9999 slots",
                d.name
            );
        }
    }

    #[test]
    fn every_kernel_and_bus_constant_is_documented_under_its_name() {
        for (code, name, _) in codes::ALL {
            let doc = get(*code).unwrap_or_else(|| panic!("{code} `{name}` has no catalog entry"));
            assert_eq!(doc.name, *name, "{code} is documented under another name");
        }
    }

    #[test]
    fn lookup_takes_any_name() {
        assert_eq!(lookup("play::ASSETS"), get(codes::PLAY_ASSETS));
        assert_eq!(lookup("ASSETS"), get(codes::PLAY_ASSETS));
        let doc = lookup("ara1302").expect("by number");
        assert_eq!(doc.name, "UNRESOLVED_REF");
        assert_eq!(lookup("1302"), Some(doc));
        assert_eq!(lookup("unresolved_ref"), Some(doc));
        assert_eq!(lookup("`ARA1302`"), Some(doc));
        let gpu = lookup("ARA_GPU_STALE_HANDLE").expect("by label");
        assert_eq!(gpu.code, codes::GPU_STALE_HANDLE);
        assert_eq!(codes::GPU_STALE_HANDLE.to_string(), "ARA_GPU_STALE_HANDLE");
        assert!(lookup("ARA9998").is_none());
    }
}
