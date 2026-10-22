//! Source-preserving authoring and typed designer edits. Runtime construction is separate.
use serde::Serialize;
use std::{collections::BTreeMap, ops::Range};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Identity is derived from an authored ID, independent of sibling order and source offsets.
pub fn stable_id(name: &str) -> u64 {
    name.bytes().fold(0xcbf29ce484222325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

#[derive(Debug)]
pub struct Element {
    pub id: String,
    pub tag: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
}

/// Parse only the declared subset; roxmltree supplies exact XML diagnostics and value spans.
pub fn parse(source: &str) -> Result<Vec<Element>> {
    let document = roxmltree::Document::parse(source)?;
    let mut elements = Vec::new();
    let mut ids = BTreeMap::new();
    for node in document.descendants().filter(|n| n.is_element()) {
        let tag = node.tag_name().name();
        if !matches!(
            tag,
            "panel" | "text" | "button" | "field" | "image" | "progress"
        ) {
            return Err(format!(
                "UI001 at {}: unknown element <{tag}>",
                document.text_pos_at(node.range().start)
            )
            .into());
        }
        let id = node
            .attribute("id")
            .ok_or("UI002: every element requires an authored id")?;
        if ids.insert(stable_id(id), id).is_some() {
            return Err(format!("UI003: duplicate ID or hash collision: {id}").into());
        }
        for attr in node.attributes() {
            if !matches!(
                attr.name(),
                "id" | "class"
                    | "text"
                    | "style"
                    | "bind"
                    | "action"
                    | "disabled"
                    | "src"
                    | "label"
            ) {
                return Err(format!(
                    "UI004 at {}: unknown attribute {}",
                    document.text_pos_at(attr.range().start),
                    attr.name()
                )
                .into());
            }
        }
        if node
            .children()
            .any(|n| n.is_text() && !n.text().unwrap_or_default().trim().is_empty())
        {
            return Err(format!(
                "UI005: <{tag}> uses a text attribute; mixed content is outside this spike subset"
            )
            .into());
        }
        let parent = node
            .parent()
            .filter(|p| p.is_element())
            .and_then(|p| p.attribute("id"))
            .and_then(|id| elements.iter().position(|e: &Element| e.id == id));
        let index = elements.len();
        elements.push(Element {
            id: id.into(),
            tag: tag.into(),
            attrs: node
                .attributes()
                .map(|a| (a.name().into(), a.value().into()))
                .collect(),
            children: Vec::new(),
            parent,
        });
        if let Some(parent) = parent {
            elements[parent].children.push(index);
        }
    }
    Ok(elements)
}

/// All authoring changes are commands producing a revision-checked operation with an inverse.
pub struct SetAttribute<'a> {
    pub id: &'a str,
    pub attribute: &'a str,
    pub value: &'a str,
    pub revision: u64,
}
#[derive(Debug, Serialize)]
pub struct SourceOp {
    pub revision: u64,
    pub bytes: Range<usize>,
    pub before: String,
    pub after: String,
}
impl SourceOp {
    /// Inversion uses the post-edit byte range and the caller's current document revision.
    pub fn inverse(&self, revision: u64) -> Self {
        Self {
            revision,
            bytes: self.bytes.start..self.bytes.start + self.after.len(),
            before: self.after.to_owned(),
            after: self.before.to_owned(),
        }
    }
}
pub struct SourceDocument {
    source: String,
    revision: u64,
}
impl SourceDocument {
    /// Validate once on load; all later source writes go through revision-checked operations.
    pub fn parse(source: &str) -> Result<Self> {
        parse(source)?;
        Ok(Self {
            source: source.into(),
            revision: 0,
        })
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn command(&self, command: SetAttribute<'_>) -> Result<SourceOp> {
        if self.revision != command.revision {
            return Err("UI006: stale designer revision".into());
        }
        if command.attribute == "id" {
            return Err("UI007: identity renames require a separate command".into());
        }
        let doc = roxmltree::Document::parse(&self.source)?;
        let node = doc
            .descendants()
            .find(|n| n.attribute("id") == Some(command.id))
            .ok_or("UI008: node not found")?;
        let attr = node
            .attributes()
            .find(|a| a.name() == command.attribute)
            .ok_or("UI009: this command edits existing attributes only")?;
        let range = attr.range_value();
        let quote = self.source.as_bytes()[range.start - 1];
        let mut value = command.value.replace('&', "&amp;").replace('<', "&lt;");
        value = value
            .replace('\r', "&#13;")
            .replace('\n', "&#10;")
            .replace('\t', "&#9;");
        value = if quote == b'\'' {
            value.replace('\'', "&apos;")
        } else {
            value.replace('"', "&quot;")
        };
        Ok(SourceOp {
            revision: self.revision,
            before: self.source[range.start..range.end].into(),
            bytes: range,
            after: value,
        })
    }
    pub fn apply(&mut self, op: &SourceOp) -> Result<()> {
        if op.revision != self.revision
            || self.source.get(op.bytes.start..op.bytes.end) != Some(op.before.as_str())
        {
            return Err("UI010: source operation conflict".into());
        }
        let mut candidate = self.source[..op.bytes.start].to_owned();
        candidate.push_str(&op.after);
        candidate.push_str(&self.source[op.bytes.end..]);
        parse(&candidate)?;
        self.source = candidate;
        self.revision += 1;
        Ok(())
    }
    pub fn serialize(&self) -> &str {
        &self.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_preserve_every_other_byte_and_reject_stale_operations() -> Result<()> {
        let input = "<!-- العربية -->\r\n<panel id='root'>\n  <button  id = 'go' text='Go &amp; play' />\n</panel>\n";
        let mut doc = SourceDocument::parse(input)?;
        assert_eq!(doc.serialize().as_bytes(), input.as_bytes());
        let op = doc.command(SetAttribute {
            id: "go",
            attribute: "text",
            value: "Let's \"go\" & play",
            revision: 0,
        })?;
        doc.apply(&op)?;
        assert_eq!(
            doc.serialize(),
            input.replace("Go &amp; play", "Let&apos;s \"go\" &amp; play")
        );
        assert!(doc.apply(&op).is_err());
        assert_eq!(parse(input)?[1].id, parse(doc.serialize())?[1].id);
        doc.apply(&op.inverse(doc.revision()))?;
        assert_eq!(doc.serialize().as_bytes(), input.as_bytes());
        Ok(())
    }
}
