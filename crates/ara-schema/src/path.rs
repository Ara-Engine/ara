//! Field paths (`Transform.position.x`, `overrides[m4tz/Light.color]`,
//! `Inventory.slots[s_7c].count`) and their resolution against the schema.
//!
//! A path starts at a record. `.name` selects a field, a component (PascalCase) or a
//! vector component (`x`/`y`/`z`/`w`, `r`/`g`/`b`/`a`); `[key]` selects a map key or a
//! keyed-collection item by `id`. Brackets may nest. Arrays are never addressed by
//! index.

use std::fmt;
use std::str::FromStr;

use ara_diag::{Diagnostic, codes, did_you_mean};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::{FieldDesc, SchemaDb, Shape, TypeDesc, ValueType};

/// A parsed field path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldPath {
    segments: Vec<Segment>,
}

/// One step of a [`FieldPath`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Segment {
    /// `.name`
    Field(String),
    /// `[key]`
    Key(String),
}

impl FieldPath {
    /// A one-segment path.
    pub fn field(name: impl Into<String>) -> Self {
        FieldPath {
            segments: vec![Segment::Field(name.into())],
        }
    }

    /// This path plus `.name`.
    pub fn child(&self, name: impl Into<String>) -> Self {
        let mut p = self.clone();
        p.segments.push(Segment::Field(name.into()));
        p
    }

    /// This path plus `[key]`.
    pub fn key(&self, key: impl Into<String>) -> Self {
        let mut p = self.clone();
        p.segments.push(Segment::Key(key.into()));
        p
    }

    /// The segments, first to last.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The first segment's name (a field or component).
    pub fn head(&self) -> &str {
        match self.segments.first() {
            Some(Segment::Field(f) | Segment::Key(f)) => f,
            None => "",
        }
    }

    /// True if `self` equals `other` or is one of its ancestors.
    pub fn is_prefix_of(&self, other: &FieldPath) -> bool {
        other.segments.starts_with(&self.segments)
    }

    /// True if one path contains the other: edits to them interfere.
    pub fn overlaps(&self, other: &FieldPath) -> bool {
        self.is_prefix_of(other) || other.is_prefix_of(self)
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl FromStr for FieldPath {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let bad = |why: &str| format!("invalid field path `{s}`: {why}");
        let mut segments = Vec::new();
        let mut rest = s;
        let mut first = true;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('[') {
                let mut depth = 1usize;
                let end = after
                    .char_indices()
                    .find(|&(_, c)| {
                        depth = match c {
                            '[' => depth + 1,
                            ']' => depth - 1,
                            _ => depth,
                        };
                        depth == 0
                    })
                    .map(|(i, _)| i)
                    .ok_or_else(|| bad("unclosed `[`"))?;
                if end == 0 {
                    return Err(bad("empty `[]`"));
                }
                segments.push(Segment::Key(after[..end].to_owned()));
                rest = &after[end + 1..];
            } else {
                let body = if first {
                    rest
                } else {
                    rest.strip_prefix('.')
                        .ok_or_else(|| bad("expected `.` or `[`"))?
                };
                let end = body.find(['.', '[']).unwrap_or(body.len());
                let name = &body[..end];
                if !is_ident(name) {
                    return Err(bad("expected a field name"));
                }
                segments.push(Segment::Field(name.to_owned()));
                rest = &body[end..];
            }
            first = false;
        }
        if !matches!(segments.first(), Some(Segment::Field(_))) {
            return Err(bad("must start with a field name"));
        }
        Ok(FieldPath { segments })
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, seg) in self.segments.iter().enumerate() {
            match seg {
                Segment::Field(name) if i == 0 => f.write_str(name)?,
                Segment::Field(name) => write!(f, ".{name}")?,
                Segment::Key(key) => write!(f, "[{key}]")?,
            }
        }
        Ok(())
    }
}

impl Serialize for FieldPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FieldPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// One physical step into JSON, produced by resolving a [`FieldPath`].
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// An object key. `fill` is materialized when the key is missing and the path
    /// continues (`None`: the key must exist, as for components).
    Key {
        /// Object key.
        key: String,
        /// Value to create when the key is missing.
        fill: Option<Value>,
    },
    /// A vector component index.
    Index(usize),
    /// A keyed-collection item, by `id`.
    Item(String),
}

/// A path resolved against the schema.
#[derive(Debug, Clone)]
pub struct Resolved<'a> {
    /// JSON steps from the record root.
    pub steps: Vec<Step>,
    /// Type of the value at the path.
    pub ty: ValueType,
    /// The innermost field, for metadata (range, deprecation). `None` for components,
    /// map items and vector components.
    pub field: Option<&'a FieldDesc>,
}

impl SchemaDb {
    /// Resolves `path` inside a record of type `record`. The head is a record field
    /// (lowercase) or, for records that carry components, a component (PascalCase).
    pub fn resolve_path<'a>(
        &'a self,
        record: &'a TypeDesc,
        path: &FieldPath,
    ) -> Result<Resolved<'a>, Diagnostic> {
        let err = |msg: String| Diagnostic::error(codes::INVALID_PATH, msg).path(path.to_string());
        let segs = path.segments();
        let Some(Segment::Field(head)) = segs.first() else {
            return Err(err(format!("path `{path}` must start with a field name")));
        };
        let more = segs.len() > 1;
        let comps = record.record_info().is_some_and(|i| i.components);
        if comps && head.starts_with(|c: char| c.is_ascii_uppercase()) {
            let comp = self.get(head).filter(|t| t.is_component()).ok_or_else(|| {
                let mut d =
                    Diagnostic::error(codes::UNKNOWN_TYPE, format!("unknown component `{head}`"))
                        .path(path.to_string());
                if let Some(s) = self
                    .lookup(head)
                    .filter(|(_, alias)| *alias)
                    .map(|(t, _)| t.name.as_str())
                    .or_else(|| self.suggest_type(head, TypeDesc::is_component))
                {
                    d.message.push_str(&format!("; did you mean `{s}`?"));
                }
                d
            })?;
            let mut resolved = Resolved {
                steps: vec![Step::Key {
                    key: head.clone(),
                    fill: None,
                }],
                ty: ValueType::Named(comp.name.clone()),
                field: None,
            };
            self.descend(&mut resolved, &segs[1..], path)?;
            return Ok(resolved);
        }
        let field = self.struct_field(record, head, path)?;
        let mut resolved = Resolved {
            steps: vec![Step::Key {
                key: head.clone(),
                fill: Some(self.fill_for(field, more)),
            }],
            ty: field.ty.clone(),
            field: Some(field),
        };
        self.descend(&mut resolved, &segs[1..], path)?;
        Ok(resolved)
    }

    /// Resolves `path` inside a plain value of type `ty` (no components).
    pub fn resolve_value_path<'a>(
        &'a self,
        ty: &ValueType,
        segs: &[Segment],
        path: &FieldPath,
    ) -> Result<Resolved<'a>, Diagnostic> {
        let mut resolved = Resolved {
            steps: Vec::new(),
            ty: ty.clone(),
            field: None,
        };
        self.descend(&mut resolved, segs, path)?;
        Ok(resolved)
    }

    fn fill_for(&self, field: &FieldDesc, more: bool) -> Value {
        match (&field.ty, more) {
            (ValueType::Option(inner), true) => self.zero(inner),
            _ => self.field_default(field),
        }
    }

    fn struct_field<'a>(
        &'a self,
        ty: &'a TypeDesc,
        name: &str,
        path: &FieldPath,
    ) -> Result<&'a FieldDesc, Diagnostic> {
        ty.field(name).ok_or_else(|| {
            let mut msg = format!("`{}` has no field `{name}`", ty.name);
            if let Some(s) = did_you_mean(name, ty.fields().iter().map(|f| f.name.as_str())) {
                msg.push_str(&format!("; did you mean `{s}`?"));
            }
            Diagnostic::error(codes::INVALID_PATH, msg).path(path.to_string())
        })
    }

    fn descend<'a>(
        &'a self,
        r: &mut Resolved<'a>,
        segs: &[Segment],
        path: &FieldPath,
    ) -> Result<(), Diagnostic> {
        let err = |msg: String| Diagnostic::error(codes::INVALID_PATH, msg).path(path.to_string());
        for (i, seg) in segs.iter().enumerate() {
            let more = i + 1 < segs.len();
            let ty = match &r.ty {
                ValueType::Option(inner) => inner.as_ref().clone(),
                other => other.clone(),
            };
            match (&ty, seg) {
                (ValueType::Named(n), Segment::Field(name)) => {
                    let desc = self
                        .get(n)
                        .filter(|t| matches!(t.shape, Shape::Struct { .. }))
                        .ok_or_else(|| err(format!("cannot select `.{name}` inside `{n}`")))?;
                    let field = self.struct_field(desc, name, path)?;
                    r.steps.push(Step::Key {
                        key: name.clone(),
                        fill: Some(self.fill_for(field, more)),
                    });
                    r.ty = field.ty.clone();
                    r.field = Some(field);
                }
                (ValueType::Json, seg) => {
                    let (Segment::Field(key) | Segment::Key(key)) = seg;
                    r.steps.push(Step::Key {
                        key: key.clone(),
                        fill: Some(Value::Object(Map::new())),
                    });
                    r.field = None;
                }
                (ValueType::Map(item), Segment::Key(key)) => {
                    let fill = match item.as_ref() {
                        ValueType::Option(inner) if more => self.zero(inner),
                        other => self.zero(other),
                    };
                    r.steps.push(Step::Key {
                        key: key.clone(),
                        fill: Some(fill),
                    });
                    r.ty = item.as_ref().clone();
                    r.field = None;
                }
                (ValueType::Keyed(item), Segment::Key(id)) => {
                    r.steps.push(Step::Item(id.clone()));
                    r.ty = item.as_ref().clone();
                    r.field = None;
                }
                (t, Segment::Field(name)) if t.vector_len().is_some() => {
                    let index = t
                        .component_index(name)
                        .ok_or_else(|| err(format!("`{t}` has no component `{name}`")))?;
                    r.steps.push(Step::Index(index));
                    r.ty = ValueType::F32;
                }
                (ValueType::List(_), _) => {
                    return Err(err(
                        "lists are replaced whole, never addressed by index".to_owned()
                    ));
                }
                (t, seg) => {
                    let sel = match seg {
                        Segment::Field(f) => format!(".{f}"),
                        Segment::Key(k) => format!("[{k}]"),
                    };
                    return Err(err(format!("cannot select `{sel}` inside a `{t}`")));
                }
            }
        }
        Ok(())
    }
}

/// Reads the stored value at `steps`; `None` if anything along the way is missing.
pub fn get<'v>(root: &'v Map<String, Value>, steps: &[Step]) -> Option<&'v Value> {
    let (first, rest) = steps.split_first()?;
    let Step::Key { key, .. } = first else {
        return None;
    };
    let mut cur = root.get(key)?;
    for step in rest {
        cur = match step {
            Step::Key { key, .. } => cur.as_object()?.get(key)?,
            Step::Index(i) => cur.as_array()?.get(*i)?,
            Step::Item(id) => cur.as_array()?.iter().find(|v| item_id(v) == Some(id))?,
        };
    }
    Some(cur)
}

/// Reads the value at `steps`, falling back to defaults for omitted fields.
pub fn get_effective(root: &Map<String, Value>, steps: &[Step]) -> Option<Value> {
    let mut cur: Option<Value> = Some(Value::Object(root.clone()));
    for step in steps {
        let container = cur?;
        cur = match step {
            Step::Key { key, fill } => match container.as_object().and_then(|o| o.get(key)) {
                Some(v) if !v.is_null() => Some(v.clone()),
                _ => fill.clone(),
            },
            Step::Index(i) => container.as_array().and_then(|a| a.get(*i)).cloned(),
            Step::Item(id) => container
                .as_array()
                .and_then(|a| a.iter().find(|v| item_id(v) == Some(id)))
                .cloned(),
        };
    }
    cur
}

fn item_id(v: &Value) -> Option<&String> {
    match v.get("id") {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

/// Writes (`Some`) or removes (`None`) the value at `steps`, materializing omitted
/// intermediate values from their defaults. Returns the previous stored value. On
/// error `root` is unchanged.
pub fn set(
    root: &mut Map<String, Value>,
    steps: &[Step],
    value: Option<Value>,
) -> Result<Option<Value>, String> {
    let Some((Step::Key { key, fill }, rest)) = steps.split_first() else {
        return Err("a path must start at a record key".to_owned());
    };
    if rest.is_empty() {
        return Ok(match value {
            Some(v) => root.insert(key.clone(), v),
            None => root.shift_remove(key),
        });
    }
    // Work on a copy of the top-level value so a failure deep inside leaves no
    // materialized intermediates behind.
    let mut slot = match root.get(key) {
        Some(v) if !v.is_null() => v.clone(),
        _ => fill
            .clone()
            .ok_or_else(|| format!("`{key}` is not present"))?,
    };
    let prev = set_in(&mut slot, rest, value)?;
    root.insert(key.clone(), slot);
    Ok(prev)
}

fn materialize<'v>(
    obj: &'v mut Map<String, Value>,
    key: &str,
    fill: Option<&Value>,
) -> Result<&'v mut Value, String> {
    let missing = obj.get(key).is_none_or(Value::is_null);
    if missing {
        let fill = fill.ok_or_else(|| format!("`{key}` is not present"))?;
        obj.insert(key.to_owned(), fill.clone());
    }
    obj.get_mut(key)
        .ok_or_else(|| format!("`{key}` is not present"))
}

fn set_in(cur: &mut Value, steps: &[Step], value: Option<Value>) -> Result<Option<Value>, String> {
    let Some((step, rest)) = steps.split_first() else {
        return Err("empty path".to_owned());
    };
    let last = rest.is_empty();
    match step {
        Step::Key { key, fill } => {
            let obj = cur
                .as_object_mut()
                .ok_or_else(|| format!("cannot select `{key}`: not an object"))?;
            if last {
                return Ok(match value {
                    Some(v) => obj.insert(key.clone(), v),
                    None => obj.shift_remove(key),
                });
            }
            set_in(materialize(obj, key, fill.as_ref())?, rest, value)
        }
        Step::Index(i) => {
            let slot = cur
                .as_array_mut()
                .and_then(|a| a.get_mut(*i))
                .ok_or_else(|| format!("vector has no component {i}"))?;
            match (last, value) {
                (true, Some(v)) => Ok(Some(std::mem::replace(slot, v))),
                (true, None) => Err("cannot remove a vector component".to_owned()),
                (false, _) => Err("cannot select inside a number".to_owned()),
            }
        }
        Step::Item(id) => {
            let items = cur
                .as_array_mut()
                .ok_or_else(|| format!("cannot select `[{id}]`: not a list"))?;
            let pos = items
                .iter()
                .position(|v| item_id(v) == Some(id))
                .ok_or_else(|| format!("no item with id `{id}`"))?;
            if !last {
                return set_in(&mut items[pos], rest, value);
            }
            Ok(Some(match value {
                Some(v) => std::mem::replace(&mut items[pos], v),
                None => items.remove(pos),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        for text in [
            "Transform.position.x",
            "overrides[m4tz/Inventory.slots[s_7c].count]",
            "Inventory.slots[s_7c].count",
            "name",
        ] {
            let p: FieldPath = text.parse().expect("valid path");
            assert_eq!(p.to_string(), text);
        }
        let p: FieldPath = "overrides[a.b[c]]".parse().expect("nested brackets");
        assert_eq!(p.segments()[1], Segment::Key("a.b[c]".into()));
        for bad in ["", ".a", "a.", "a..b", "[k]", "a[", "a[]", "a.1b"] {
            assert!(bad.parse::<FieldPath>().is_err(), "{bad}");
        }
    }

    #[test]
    fn prefixes_overlap() {
        let a: FieldPath = "Transform".parse().expect("valid");
        let b: FieldPath = "Transform.position".parse().expect("valid");
        let c: FieldPath = "Transform.scale".parse().expect("valid");
        assert!(a.overlaps(&b) && b.overlaps(&a));
        assert!(!b.overlaps(&c));
    }
}
