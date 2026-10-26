use crate::{Diagnostic, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Stable authored/model-key identity, independent of source offset and pool slot.
pub fn stable_id(name: &str) -> u64 {
    name.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
/// One compiled element; indexes address the same flat document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Element {
    /// Authored ID, prefixed with instance identity inside components.
    pub id: String,
    /// Registered primitive name.
    pub tag: String,
    /// Validated authored attributes.
    pub attrs: BTreeMap<String, String>,
    /// Ordered child indexes.
    pub children: Vec<usize>,
    /// Parent index, absent for the root.
    pub parent: Option<usize>,
}
/// All primitives accepted by markup and type selectors.
pub const TAGS: &[&str] = &[
    "ui", "panel", "text", "image", "button", "toggle", "slider", "field", "input", "scroll",
    "splitter", "tabs", "tab", "dropdown", "menu", "popup", "tooltip", "modal", "progress", "list",
    "tree", "table", "dock", "template", "instance", "slot",
];
/// Attributes shared by native controls; typed widget values are validated by the runtime.
pub const ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "text",
    "style",
    "bind",
    "action",
    "disabled",
    "src",
    "tint",
    "label",
    "description",
    "cursor",
    "active-cursor",
    "test-id",
    "value",
    "min",
    "max",
    "step",
    "checked",
    "selected",
    "template",
    "slot",
    "name",
    "tab-index",
    "nav-left",
    "nav-right",
    "nav-up",
    "nav-down",
    "expanded",
];
/// Parse the strict XML-shaped authoring subset without altering original bytes.
pub fn parse(source: &str) -> Result<Vec<Element>> {
    let doc = roxmltree::Document::parse(source).map_err(|e| {
        let pos = e.pos();
        let byte = source
            .split_inclusive('\n')
            .take(pos.row.saturating_sub(1) as usize)
            .map(str::len)
            .sum::<usize>();
        let column = source[byte..]
            .split('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .take(pos.col.saturating_sub(1) as usize)
            .map(char::len_utf8)
            .sum::<usize>();
        let byte = byte + column;
        Diagnostic::new(
            crate::codes::MARKUP,
            source,
            byte..byte,
            e.to_string(),
            "Use well-formed markup, quoted attributes and explicit IDs",
        )
    })?;
    let mut out: Vec<Element> = Vec::new();
    // Each element's ancestor count (itself and the document node included).
    let mut depths: Vec<usize> = Vec::new();
    let mut ids = BTreeSet::new();
    let mut indexes = std::collections::HashMap::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        let bad = |code, msg: String, fix| Diagnostic::new(code, source, node.range(), msg, fix);
        let tag = node.tag_name().name();
        if node.tag_name().namespace().is_some() || !TAGS.contains(&tag) {
            return Err(bad(
                crate::codes::ELEMENT,
                format!("Unsupported element {tag}"),
                "Use a registered native UI primitive",
            ));
        }
        let parent = node.parent().and_then(|p| indexes.get(&p.id()).copied());
        let depth = parent.map_or(2, |p: usize| depths[p] + 1);
        if depth > 128 {
            return Err(bad(
                crate::codes::DEPTH,
                "Markup nesting exceeds 128".into(),
                "Split the document into components",
            ));
        }
        let id = node
            .attribute("id")
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                bad(
                    crate::codes::MISSING_ID,
                    "Missing authored ID".into(),
                    "Add a unique id attribute",
                )
            })?;
        if !ids.insert(stable_id(id)) {
            return Err(bad(
                crate::codes::DUPLICATE_ID,
                format!("Duplicate ID or hash collision: {id}"),
                "Choose a unique authored ID",
            ));
        }
        for attr in node.attributes() {
            if matches!(attr.name(), "disabled" | "checked" | "expanded")
                && !matches!(attr.value(), "true" | "false")
            {
                return Err(Diagnostic::new(
                    crate::codes::ATTRIBUTE,
                    source,
                    attr.range(),
                    "Invalid boolean attribute",
                    "Use true or false",
                ));
            }
            if attr.name() == "tab-index" && attr.value().parse::<i32>().is_err() {
                return Err(Diagnostic::new(
                    crate::codes::ATTRIBUTE,
                    source,
                    attr.range(),
                    "Invalid tab index",
                    "Use an integer; negative values opt out of tab order",
                ));
            }
            if attr.namespace().is_some() || !ATTRIBUTES.contains(&attr.name()) {
                return Err(Diagnostic::new(
                    crate::codes::ATTRIBUTE,
                    source,
                    attr.range(),
                    format!("Unsupported attribute {}", attr.name()),
                    "Use a documented native attribute",
                ));
            }
        }
        if node
            .children()
            .any(|n| n.is_text() && !n.text().unwrap_or_default().trim().is_empty())
        {
            return Err(bad(
                crate::codes::MIXED_TEXT,
                "Mixed text content is unsupported".into(),
                "Place text in the text attribute",
            ));
        }
        let index = out.len();
        indexes.insert(node.id(), index);
        depths.push(depth);
        out.push(Element {
            id: id.into(),
            tag: tag.into(),
            attrs: node
                .attributes()
                .map(|a| (a.name().into(), a.value().into()))
                .collect(),
            children: Vec::new(),
            parent,
        });
        if let Some(p) = parent {
            out[p].children.push(index);
        }
    }
    Ok(out)
}
/// One inline style with the exact original attribute range for diagnostics.
pub struct InlineStyle {
    /// Source node identity.
    pub id: String,
    /// XML-decoded style declarations.
    pub value: String,
    /// Original attribute value byte range, including any XML entities.
    pub bytes: std::ops::Range<usize>,
}
/// Extract inline styles after validating markup, preserving precise source attribution.
pub fn inline_styles(source: &str) -> Result<Vec<InlineStyle>> {
    parse(source)?;
    let doc = roxmltree::Document::parse(source).expect("validated markup remains valid XML");
    Ok(doc
        .descendants()
        .filter(|n| n.is_element())
        .filter_map(|n| {
            n.attributes()
                .find(|a| a.name() == "style")
                .map(|a| InlineStyle {
                    id: n
                        .attribute("id")
                        .expect("all validated nodes have IDs")
                        .into(),
                    value: a.value().into(),
                    bytes: a.range_value(),
                })
        })
        .collect())
}
/// Expand templates and named slots with deterministic instance-prefixed IDs.
pub fn compile(source: &str) -> Result<Vec<Element>> {
    let input = parse(source)?;
    // Without components there is nothing to expand: `parse` already checked
    // identities, and the expansion below would rebuild the same elements.
    if !input
        .iter()
        .any(|e| matches!(e.tag.as_str(), "template" | "instance" | "slot"))
    {
        return Ok(input);
    }
    let templates: BTreeMap<_, _> = input
        .iter()
        .enumerate()
        .filter(|(_, e)| e.tag == "template")
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();
    let mut out = Vec::new();
    #[allow(
        clippy::too_many_arguments,
        reason = "Recursive source expansion passes source and target ownership explicitly"
    )]
    fn visit(
        input: &[Element],
        i: usize,
        parent: Option<usize>,
        prefix: &str,
        slots: Option<&BTreeMap<&str, usize>>,
        templates: &BTreeMap<&str, usize>,
        out: &mut Vec<Element>,
        depth: usize,
    ) -> Result<()> {
        let error = || {
            Diagnostic::new(
                crate::codes::COMPONENT,
                "",
                0..0,
                "Invalid component, recursive template or missing slot",
                "Use an existing template with one root and supply each named slot",
            )
        };
        if depth > 128 || out.len() > 1_000_000 {
            return Err(error());
        }
        let e = &input[i];
        if e.tag == "template" {
            return Ok(());
        }
        if e.tag == "instance" {
            let t = *templates
                .get(
                    e.attrs
                        .get("template")
                        .map(String::as_str)
                        .ok_or_else(error)?,
                )
                .ok_or_else(error)?;
            if input[t].children.len() != 1 {
                return Err(error());
            }
            let mut supplied = BTreeMap::new();
            for c in &e.children {
                let name = input[*c]
                    .attrs
                    .get("slot")
                    .map(String::as_str)
                    .unwrap_or("default");
                if supplied.insert(name, *c).is_some() {
                    return Err(error());
                }
            }
            return visit(
                input,
                input[t].children[0],
                parent,
                &format!("{prefix}{}/", e.id),
                Some(&supplied),
                templates,
                out,
                depth + 1,
            );
        }
        if e.tag == "slot" {
            let name = e.attrs.get("name").map(String::as_str).unwrap_or("default");
            if let Some(c) = slots.and_then(|s| s.get(name)) {
                return visit(input, *c, parent, prefix, None, templates, out, depth + 1);
            }
            for c in &e.children {
                visit(input, *c, parent, prefix, None, templates, out, depth + 1)?;
            }
            return Ok(());
        }
        let index = out.len();
        let mut element = e.clone();
        element.id = format!("{prefix}{}", e.id);
        element.attrs.insert("id".into(), element.id.clone());
        element.children.clear();
        element.parent = parent;
        out.push(element);
        if let Some(p) = parent {
            out[p].children.push(index);
        }
        for child in &e.children {
            visit(
                input,
                *child,
                Some(index),
                prefix,
                slots,
                templates,
                out,
                depth + 1,
            )?;
        }
        Ok(())
    }
    visit(&input, 0, None, "", None, &templates, &mut out, 0)?;
    let mut ids = BTreeSet::new();
    if out.is_empty() || out.iter().any(|e| !ids.insert(stable_id(&e.id))) {
        return Err(Diagnostic::new(
            crate::codes::DUPLICATE_ID,
            source,
            0..source.len(),
            "Expanded identity collision or empty root",
            "Use unique instance and slot IDs",
        ));
    }
    Ok(out)
}
/// Escape attribute content while preserving the original quote delimiter.
pub fn escape(value: &str, quote: u8) -> String {
    let mut s = value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
        .replace('\t', "&#9;");
    s = if quote == b'\'' {
        s.replace('\'', "&apos;")
    } else {
        s.replace('"', "&quot;")
    };
    s
}
