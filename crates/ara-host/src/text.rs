//! Scene-to-text (R03 §4.5): the token-tuned summary `inspect` and `query` return
//! by default.
//!
//! ```text
//! scene arena (sc_arena001) · levels/arena.scene · 8 entities · showing 1-6 of 8, depth≤2 · units m/deg · Y-up
//! e_flr1 Floor pos(0,0,0) scale(40,0.5,40)
//! e_stk2 Crates prefab:stack pos(4,0,-2)
//!   e_stk2/e_c001 Crate [MeshRenderer] pos(0,0,0)
//! … 2 more · next: {"cursor":"c6"}
//! ```
//!
//! `Transform` is shown as `pos`/`rot`/`scale` (rot and scale only when not
//! identity); other components are listed in brackets (omitted when none).

use ara_doc::Workspace;
use ara_doc::prefab::ResolvedEntity;
use ara_schema::{SchemaDb, Shape};
use serde_json::{Map, Value};

use crate::tree::Tree;

/// `(1,0.5,3)` from a JSON number array; `None` if it isn't one.
fn tuple(v: &Value) -> Option<String> {
    let items = v.as_array()?;
    let parts: Option<Vec<String>> = items
        .iter()
        .map(|n| n.as_f64().map(ara_json::format_f64))
        .collect();
    Some(format!("({})", parts?.join(",")))
}

/// A compact value: tuples for number arrays, bare strings, JSON otherwise.
pub fn short(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => ara_json::format_number(n),
        Value::Array(_) => tuple(v).unwrap_or_else(|| v.to_string()),
        _ => v.to_string(),
    }
}

/// Every field of a component, stored or default, with its unit symbol.
pub fn effective(
    db: &SchemaDb,
    component: &str,
    value: &Value,
) -> Vec<(String, Value, &'static str)> {
    let stored = value.as_object().cloned().unwrap_or_else(Map::new);
    match db.get(component).map(|t| &t.shape) {
        Some(Shape::Struct { fields, .. }) => fields
            .iter()
            .map(|f| {
                let v = stored
                    .get(&f.name)
                    .cloned()
                    .unwrap_or_else(|| db.field_default(f));
                (f.name.clone(), v, f.unit.map_or("", |u| u.symbol()))
            })
            .collect(),
        _ => stored.into_iter().map(|(k, v)| (k, v, "")).collect(),
    }
}

/// The one-line form of an entity (without indentation).
pub fn entity_line(db: &SchemaDb, addr: &str, e: &ResolvedEntity, prefab: Option<&str>) -> String {
    let name = e.record.get("name").and_then(Value::as_str).unwrap_or("-");
    let mut line = format!("{addr} {name}");
    if let Some(p) = prefab {
        line.push_str(&format!(" prefab:{p}"));
    }
    let comps: Vec<&str> = e
        .record
        .keys()
        .filter(|k| ara_doc::is_component_key(k) && *k != "Transform")
        .map(String::as_str)
        .collect();
    if !comps.is_empty() {
        line.push_str(&format!(" [{}]", comps.join(" ")));
    }
    if let Some(t) = e.record.get("Transform") {
        for (field, v, _) in effective(db, "Transform", t) {
            let is_identity = match field.as_str() {
                "rotation" => v
                    .as_array()
                    .is_some_and(|a| a.iter().all(|n| n.as_f64() == Some(0.0))),
                "scale" => v
                    .as_array()
                    .is_some_and(|a| a.iter().all(|n| n.as_f64() == Some(1.0))),
                _ => false,
            };
            let label = match field.as_str() {
                "position" => "pos",
                "rotation" => "rot",
                other => other,
            };
            if !is_identity {
                line.push_str(&format!(" {label}{}", short(&v)));
            }
        }
    }
    line
}

/// The hierarchy summary of a document, `limit` lines from `offset`, entities no
/// deeper than `depth`. Returns the text and the offset of the next page.
pub fn summary(
    ws: &Workspace,
    db: &SchemaDb,
    tree: &Tree,
    depth: u32,
    offset: usize,
    limit: usize,
) -> (String, Option<usize>) {
    let visible: Vec<&(String, u32)> = tree.order.iter().filter(|(_, d)| *d <= depth).collect();
    let total = tree.entities.len();
    let end = (offset + limit).min(visible.len());
    let shown = if offset < end {
        format!("showing {}-{end} of {}", offset + 1, visible.len())
    } else {
        format!("showing 0 of {}", visible.len())
    };
    let mut out = format!(
        "{} {} ({}) · {} · {total} entities · {shown}, depth≤{depth} · units m/deg · Y-up\n",
        tree.kind,
        tree.name(),
        tree.doc,
        tree.path
    );
    for (addr, d) in visible.get(offset..end).unwrap_or_default() {
        if let Some(e) = tree.get(addr) {
            let prefab = tree.prefab_name(ws, addr);
            out.push_str(&"  ".repeat(*d as usize));
            out.push_str(&entity_line(db, addr, e, prefab.as_deref()));
            out.push('\n');
        }
    }
    let next = (end < visible.len()).then_some(end);
    if let Some(n) = next {
        out.push_str(&format!(
            "… {} more · next: {{\"cursor\":\"c{n}\"}}\n",
            visible.len() - n
        ));
    }
    (out, next)
}

/// Every component field of one entity, effective values with units.
pub fn details(ws: &Workspace, db: &SchemaDb, tree: &Tree, addr: &str) -> Option<String> {
    let e = tree.get(addr)?;
    let mut out = entity_line(db, addr, e, tree.prefab_name(ws, addr).as_deref());
    if let Some(p) = tree.parent(addr) {
        out.push_str(&format!(" · parent {p}"));
    }
    out.push_str(&format!(" · defined in {}#{}\n", e.source.0, e.source.1));
    for (name, value) in &e.record {
        if !ara_doc::is_component_key(name) {
            continue;
        }
        let fields: Vec<String> = effective(db, name, value)
            .into_iter()
            .map(|(f, v, unit)| format!("{f}={}{unit}", short(&v)))
            .collect();
        out.push_str(&format!("  {name}: {}\n", fields.join(" ")));
    }
    Some(out)
}
