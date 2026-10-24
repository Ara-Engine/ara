use crate::{Diagnostic, Result, escape, parse, stylesheet::Stylesheet};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Source grammar used to validate an atomic operation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum DocumentKind {
    /// Native `.ui` markup.
    Markup,
    /// Restricted `.css` stylesheet.
    Stylesheet,
}
/// Typed authoring intent; commands never mutate source directly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    /// Change/add an attribute, or remove it with `None`.
    SetAttribute {
        /// Target authored ID.
        id: String,
        /// Attribute name.
        name: String,
        /// New logical value, escaped by the writer.
        value: Option<String>,
    },
    /// Insert one exact markup fragment before a child, or at the end.
    Insert {
        /// Parent authored ID.
        parent: String,
        /// Next sibling, if any.
        before: Option<String>,
        /// New source fragment.
        source: String,
    },
    /// Remove the node's bytes; adjacent comments and whitespace remain untouched.
    Remove {
        /// Authored ID.
        id: String,
    },
    /// Move the original node bytes without reserializing them.
    Move {
        /// Authored ID.
        id: String,
        /// Destination parent.
        parent: String,
        /// Destination next sibling.
        before: Option<String>,
    },
    /// Change/add a declaration by source-order rule index.
    SetStyle {
        /// Rule index (including :root).
        rule: usize,
        /// Property/token name.
        property: String,
        /// New declaration value.
        value: String,
    },
}
/// One byte patch, addressed in the pre-operation source.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Patch {
    /// Expected range.
    pub bytes: Range<usize>,
    /// Expected exact source bytes.
    pub before: String,
    /// Replacement source bytes.
    pub after: String,
}
/// Atomic, revision-checked operation. Patches are nonoverlapping and source-ordered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceOp {
    /// Expected revision.
    pub revision: u64,
    /// All byte patches in this transaction.
    pub patches: Vec<Patch>,
}
impl SourceOp {
    /// Produce inverse patches in post-operation coordinates.
    pub fn inverse(&self, revision: u64) -> Self {
        let mut delta = 0isize;
        let patches = self
            .patches
            .iter()
            .map(|p| {
                let start = p.bytes.start.saturating_add_signed(delta);
                delta += p.after.len() as isize - p.before.len() as isize;
                Patch {
                    bytes: start..start + p.after.len(),
                    before: p.after.clone(),
                    after: p.before.clone(),
                }
            })
            .collect();
        Self { revision, patches }
    }
}
/// Original UTF-8 source plus monotonic revision; its writer preserves unrelated bytes.
pub struct SourceDocument {
    source: String,
    revision: u64,
    kind: DocumentKind,
}
impl SourceDocument {
    /// Load and validate a document without normalizing its formatting.
    pub fn parse(source: &str, kind: DocumentKind) -> Result<Self> {
        Self::validate(source, kind)?;
        Ok(Self {
            source: source.into(),
            revision: 0,
            kind,
        })
    }
    fn validate(source: &str, kind: DocumentKind) -> Result<()> {
        match kind {
            DocumentKind::Markup => {
                parse(source)?;
            }
            DocumentKind::Stylesheet => {
                Stylesheet::parse(source)?;
            }
        }
        Ok(())
    }
    /// Exact authored bytes, also used for no-op saves.
    pub fn serialize(&self) -> &str {
        &self.source
    }
    /// Current operation revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    fn error(&self, message: &str) -> Diagnostic {
        Diagnostic::new(
            crate::codes::EDIT,
            &self.source,
            0..0,
            message,
            "Reload the current source and regenerate a typed command",
        )
    }
    /// Compile an authoring command to validated, reversible patches.
    pub fn command(&self, revision: u64, command: Command) -> Result<SourceOp> {
        if revision != self.revision {
            return Err(self.error("Stale source revision"));
        }
        let patch = |bytes: Range<usize>, after: String| Patch {
            before: self.source[bytes.clone()].into(),
            bytes,
            after,
        };
        let mut patches = Vec::new();
        if let Command::SetStyle {
            rule,
            property,
            value,
        } = command
        {
            if !matches!(self.kind, DocumentKind::Stylesheet) {
                return Err(self.error("Style edit requires a stylesheet"));
            }
            let sheet = Stylesheet::parse(&self.source)?;
            let rule = sheet
                .rules
                .get(rule)
                .ok_or_else(|| self.error("Rule not found"))?;
            if let Some(d) = rule.declarations.iter().rev().find(|d| d.name == property) {
                patches.push(patch(d.bytes.clone(), value));
            } else {
                let separator = if self.source[..rule.end].trim_end().ends_with([';', '{']) {
                    ""
                } else {
                    ";"
                };
                patches.push(patch(
                    rule.end..rule.end,
                    format!("{separator} {property}: {value};"),
                ));
            }
        } else {
            if !matches!(self.kind, DocumentKind::Markup) {
                return Err(self.error("Markup edit requires markup"));
            }
            let doc = roxmltree::Document::parse(&self.source)
                .map_err(|_| self.error("Invalid source"))?;
            let find = |id: &str| {
                doc.descendants()
                    .find(|n| n.is_element() && n.attribute("id") == Some(id))
                    .ok_or_else(|| self.error("Node not found"))
            };
            let insertion = |parent: &str,
                             before: &Option<String>|
             -> Result<(Range<usize>, String)> {
                let p = find(parent)?;
                if let Some(before) = before {
                    let b = find(before)?;
                    if b.parent() != Some(p) {
                        return Err(self.error("Destination sibling is not a child of the parent"));
                    }
                    return Ok((b.range().start..b.range().start, String::new()));
                }
                let range = p.range();
                let raw = &self.source[range.clone()];
                if let Some(end) = raw.rfind("</") {
                    let at = range.start + end;
                    Ok((at..at, String::new()))
                } else {
                    let at = range.end - 2;
                    Ok((at..range.end, format!("></{}>", p.tag_name().name())))
                }
            };
            match command {
                Command::SetAttribute { id, name, value } => {
                    if name == "id" {
                        return Err(self.error("Identity renames require reference migration"));
                    }
                    let node = find(&id)?;
                    if let Some(attr) = node.attributes().find(|a| a.name() == name) {
                        if let Some(value) = value {
                            let r = attr.range_value();
                            patches.push(patch(
                                r.clone(),
                                escape(&value, self.source.as_bytes()[r.start - 1]),
                            ));
                        } else {
                            patches.push(patch(attr.range(), String::new()));
                        }
                    } else if let Some(value) = value {
                        let at = node.range().start + 1 + node.tag_name().name().len();
                        patches.push(patch(
                            at..at,
                            format!(" {name}=\"{}\"", escape(&value, b'"')),
                        ));
                    }
                }
                Command::Remove { id } => {
                    let n = find(&id)?;
                    if n.parent_element().is_none() {
                        return Err(self.error("Cannot remove the document root"));
                    }
                    patches.push(patch(n.range(), String::new()));
                }
                Command::Insert {
                    parent,
                    before,
                    source,
                } => {
                    parse(&source)?;
                    let (range, suffix) = insertion(&parent, &before)?;
                    let after = if suffix.is_empty() {
                        source
                    } else {
                        format!(">{source}{}", &suffix[1..])
                    };
                    patches.push(patch(range, after));
                }
                Command::Move { id, parent, before } => {
                    let n = find(&id)?;
                    let p = find(&parent)?;
                    if p.ancestors().any(|a| a == n)
                        || n.parent_element().is_none()
                        || before.as_deref() == Some(&id)
                    {
                        return Err(self.error("Cannot move a root or move into its own subtree"));
                    }
                    let (range, suffix) = insertion(&parent, &before)?;
                    let raw = &self.source[n.range()];
                    let after = if suffix.is_empty() {
                        raw.into()
                    } else {
                        format!(">{raw}{}", &suffix[1..])
                    };
                    patches.push(patch(n.range(), String::new()));
                    patches.push(patch(range, after));
                }
                Command::SetStyle { .. } => unreachable!("handled before markup parsing"),
            }
        }
        patches.sort_by_key(|p| p.bytes.start);
        let op = SourceOp { revision, patches };
        self.candidate(&op)?;
        Ok(op)
    }
    fn candidate(&self, op: &SourceOp) -> Result<String> {
        if op.revision != self.revision {
            return Err(self.error("Stale source operation"));
        }
        let mut end = 0;
        let mut out = String::with_capacity(self.source.len());
        for p in &op.patches {
            if p.bytes.start < end
                || p.bytes.end < p.bytes.start
                || self.source.get(p.bytes.clone()) != Some(&p.before)
            {
                return Err(self.error("Conflicting source bytes or overlapping patches"));
            }
            out.push_str(&self.source[end..p.bytes.start]);
            out.push_str(&p.after);
            end = p.bytes.end;
        }
        out.push_str(&self.source[end..]);
        Self::validate(&out, self.kind)?;
        Ok(out)
    }
    /// Apply all patches atomically, validating the resulting grammar before publication.
    pub fn apply(&mut self, op: &SourceOp) -> Result<()> {
        let candidate = self.candidate(op)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| self.error("Revision exhausted"))?;
        self.source = candidate;
        self.revision = revision;
        Ok(())
    }
}
