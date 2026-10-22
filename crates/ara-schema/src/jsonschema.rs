//! JSON Schema 2020-12 generation, per type and per document kind.
//!
//! Schemas are self-contained: referenced types live under `$defs`. Docs become
//! `description`, units become `x-unit`, and enum variants become `oneOf` consts so
//! editors show each variant's doc.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::{DocKindDesc, FieldDesc, SchemaDb, Shape, TypeDesc, ValueType};

/// The JSON Schema dialect every generated schema declares.
pub const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

impl SchemaDb {
    /// The JSON Schema of one type, with its dependencies under `$defs`.
    pub fn type_json_schema(&self, t: &TypeDesc) -> Value {
        let mut root = Map::new();
        root.insert("$schema".into(), DIALECT.into());
        root.insert("$id".into(), format!("ara://schema/type/{}", t.name).into());
        root.insert("title".into(), t.name.clone().into());
        if let Value::Object(body) = self.type_body(t) {
            root.extend(body);
        }
        let defs = self.defs(self.deps_of(t));
        if !defs.is_empty() {
            root.insert("$defs".into(), Value::Object(defs));
        }
        Value::Object(root)
    }

    /// The JSON Schema of a document kind (`scene.schema.json`).
    pub fn doc_json_schema(&self, kind: &DocKindDesc) -> Value {
        let mut records = Map::new();
        let mut deps = BTreeSet::new();
        for t in kind.records.iter().filter_map(|r| self.get(r)) {
            if let Some(info) = t.record_info() {
                records.insert(
                    format!("^{}_[0-9a-z]{{1,24}}$", info.prefix),
                    json!({ "$ref": format!("#/$defs/{}", t.name) }),
                );
                deps.insert(t.name.clone());
                deps.extend(self.deps_of(t));
            }
        }
        let mut root = Map::new();
        root.insert("$schema".into(), DIALECT.into());
        root.insert("$id".into(), kind.schema_uri().into());
        root.insert("title".into(), kind.name.clone().into());
        root.insert("description".into(), kind.doc.clone().into());
        root.insert("type".into(), "object".into());
        root.insert("required".into(), json!(["$schema", "id", "records"]));
        root.insert(
            "properties".into(),
            json!({
                "$schema": { "const": kind.schema_uri() },
                "id": {
                    "description": "Project-unique asset ID of this document.",
                    "type": "string",
                    "pattern": format!("^{}_[0-9a-z]{{1,24}}$", kind.id_prefix),
                },
                "versions": {
                    "description": "Stored version of each type newer than version 1.",
                    "type": "object",
                    "additionalProperties": { "type": "integer", "minimum": 1 },
                },
                "records": {
                    "description": "Records keyed by ID. The ID prefix selects the record type.",
                    "type": "object",
                    "patternProperties": records,
                    "additionalProperties": false,
                },
            }),
        );
        root.insert("additionalProperties".into(), false.into());
        root.insert("$defs".into(), Value::Object(self.defs(deps)));
        Value::Object(root)
    }

    /// Named types reachable from `t` (including components, for records that carry
    /// them), excluding `t`.
    fn deps_of(&self, t: &TypeDesc) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<String> = self.direct_deps(t);
        while let Some(n) = stack.pop() {
            if n == t.name || !seen.insert(n.clone()) {
                continue;
            }
            if let Some(dep) = self.get(&n) {
                stack.extend(self.direct_deps(dep));
            }
        }
        seen
    }

    fn direct_deps(&self, t: &TypeDesc) -> Vec<String> {
        let mut out: Vec<String> = t
            .fields()
            .iter()
            .flat_map(|f| f.ty.named())
            .map(str::to_owned)
            .collect();
        if t.record_info().is_some_and(|i| i.components) {
            out.extend(self.components().map(|c| c.name.clone()));
        }
        out
    }

    fn defs(&self, names: BTreeSet<String>) -> Map<String, Value> {
        names
            .into_iter()
            .filter_map(|n| self.get(&n).map(|t| (n, self.type_body(t))))
            .collect()
    }

    fn type_body(&self, t: &TypeDesc) -> Value {
        let mut body = Map::new();
        body.insert("description".into(), t.doc.clone().into());
        if t.deprecated.is_some() {
            body.insert("deprecated".into(), true.into());
        }
        match &t.shape {
            Shape::Enum { variants } => {
                body.insert("type".into(), "string".into());
                let one_of: Vec<Value> = variants
                    .iter()
                    .map(|v| json!({ "const": v.name, "description": v.doc }))
                    .collect();
                body.insert("oneOf".into(), Value::Array(one_of));
            }
            Shape::Struct { fields, .. } => {
                body.insert("type".into(), "object".into());
                let mut props: Map<String, Value> = fields
                    .iter()
                    .map(|f| (f.name.clone(), self.field_schema(f)))
                    .collect();
                if t.record_info().is_some_and(|i| i.components) {
                    for c in self.components() {
                        props.insert(
                            c.name.clone(),
                            json!({ "$ref": format!("#/$defs/{}", c.name) }),
                        );
                    }
                }
                body.insert("properties".into(), Value::Object(props));
                let required: Vec<Value> = fields
                    .iter()
                    .filter(|f| f.required)
                    .map(|f| f.name.clone().into())
                    .collect();
                if !required.is_empty() {
                    body.insert("required".into(), Value::Array(required));
                }
                body.insert("additionalProperties".into(), false.into());
            }
        }
        Value::Object(body)
    }

    fn field_schema(&self, f: &FieldDesc) -> Value {
        let mut s = match self.value_schema(&f.ty) {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        s.insert("description".into(), f.doc.clone().into());
        if !f.required {
            s.insert("default".into(), self.field_default(f));
        }
        if let Some(r) = f.range {
            let target = if f.ty.vector_len().is_some() {
                s.get_mut("items").and_then(Value::as_object_mut)
            } else {
                Some(&mut s)
            };
            if let Some(t) = target {
                if let Some(min) = r.min {
                    t.insert("minimum".into(), min.into());
                }
                if let Some(max) = r.max {
                    t.insert("maximum".into(), max.into());
                }
            }
        }
        if !f.examples.is_empty() {
            s.insert("examples".into(), Value::Array(f.examples.clone()));
        }
        if f.deprecated.is_some() {
            s.insert("deprecated".into(), true.into());
        }
        if let Some(u) = f.unit {
            s.insert("x-unit".into(), u.symbol().into());
        }
        Value::Object(s)
    }

    fn value_schema(&self, ty: &ValueType) -> Value {
        let nullable_string =
            |pattern: &str| json!({ "type": ["string", "null"], "pattern": pattern });
        match ty {
            ValueType::Bool => json!({ "type": "boolean" }),
            ValueType::F32 | ValueType::F64 => json!({ "type": "number" }),
            ValueType::String => json!({ "type": "string" }),
            ValueType::Entity => {
                nullable_string(r"^[a-z]{1,3}_[0-9a-z]{1,24}(/[a-z]{1,3}_[0-9a-z]{1,24})*$")
            }
            ValueType::Asset(_) => nullable_string(r"^@[^#]*(#[a-z]{1,3}_[0-9a-z]{1,24})?$"),
            ValueType::Json => json!({}),
            ValueType::Named(n) => json!({ "$ref": format!("#/$defs/{n}") }),
            ValueType::Option(inner) => {
                json!({ "anyOf": [self.value_schema(inner), { "type": "null" }] })
            }
            ValueType::List(item) | ValueType::Keyed(item) => {
                json!({ "type": "array", "items": self.value_schema(item) })
            }
            ValueType::Map(item) => {
                json!({ "type": "object", "additionalProperties": self.value_schema(item) })
            }
            t => match (t.int_bounds(), t.vector_len()) {
                (Some((lo, hi)), _) => {
                    // i128 bounds exceed JSON numbers only for u64/i64; leave those open.
                    let mut m = json!({ "type": "integer" });
                    if let (Ok(lo), Ok(hi)) = (i64::try_from(lo), i64::try_from(hi))
                        && hi < i64::MAX
                    {
                        m["minimum"] = lo.into();
                        m["maximum"] = hi.into();
                    }
                    m
                }
                (None, Some(n)) => {
                    json!({ "type": "array", "items": { "type": "number" }, "minItems": n, "maxItems": n })
                }
                _ => json!({}),
            },
        }
    }
}
