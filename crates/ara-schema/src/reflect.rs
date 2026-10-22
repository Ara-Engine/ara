//! `reflection.json` export and data-only type definitions.
//!
//! One JSON shape serves both directions: the `types` entries of `reflection.json` are
//! exactly the format of runtime type definitions (`*.types.json`), so any exported
//! type can be fed back in.
//!
//! ```json
//! {
//!   "$schema": "ara://schema/types/1",
//!   "types": {
//!     "Health": {
//!       "kind": "component",
//!       "doc": "Hit points of a damageable entity.",
//!       "fields": {
//!         "max": { "type": "f32", "doc": "Maximum hit points.", "default": 100, "range": [1, 10000] }
//!       }
//!     }
//!   }
//! }
//! ```

use ara_diag::{Diagnostic, codes};
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::{
    DocKindDesc, EditorHints, FieldDesc, Range, RecordInfo, Replication, SchemaDb, Shape,
    StructRole, TypeDesc, Unit, ValueType, VariantDesc,
};

/// `$schema` of `reflection.json`.
pub const REFLECTION_SCHEMA: &str = "ara://schema/reflection/1";
/// `$schema` of a data-only type definition file.
pub const TYPES_SCHEMA: &str = "ara://schema/types/1";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TypeDef {
    kind: String,
    doc: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    module: String,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deprecated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefix: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    components: bool,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    fields: IndexMap<String, FieldDef>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    variants: IndexMap<String, VariantDef>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FieldDef {
    #[serde(rename = "type")]
    ty: String,
    doc: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    default: Option<Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unit: Option<Unit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<(Option<f64>, Option<f64>)>,
    /// Derived from the enum type on export, for readers; ignored on import.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    options: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    examples: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    since: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deprecated: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    replicate: Replication,
    #[serde(default, skip_serializing_if = "is_default")]
    editor: EditorHints,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VariantDef {
    doc: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deprecated: Option<String>,
}

fn one() -> u32 {
    1
}

fn is_one(v: &u32) -> bool {
    *v == 1
}

fn is_false(v: &bool) -> bool {
    !v
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Deserializes a present value (including `null`) as `Some`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

impl SchemaDb {
    /// The machine-readable Schema DB: every type and document kind.
    /// Command registries add a `commands` section.
    pub fn reflection(&self) -> Value {
        let types: Map<String, Value> = self
            .types()
            .map(|t| (t.name.clone(), self.type_json(t)))
            .collect();
        let kinds: Map<String, Value> = self
            .kinds()
            .map(|k| (k.name.clone(), kind_json(k)))
            .collect();
        let mut root = Map::new();
        root.insert("$schema".into(), REFLECTION_SCHEMA.into());
        root.insert("types".into(), Value::Object(types));
        root.insert("documents".into(), Value::Object(kinds));
        Value::Object(root)
    }

    /// One type in `reflection.json` (and type-definition) form.
    pub fn type_json(&self, t: &TypeDesc) -> Value {
        let (prefix, components) = match t.record_info() {
            Some(info) => (Some(info.prefix.clone()), info.components),
            None => (None, false),
        };
        let def = TypeDef {
            kind: t.kind_name().to_owned(),
            doc: t.doc.clone(),
            module: t.module.clone(),
            version: t.version,
            aliases: t.aliases.clone(),
            deprecated: t.deprecated.clone(),
            prefix,
            components,
            fields: t
                .fields()
                .iter()
                .map(|f| (f.name.clone(), self.field_def(f)))
                .collect(),
            variants: t
                .variants()
                .iter()
                .map(|v| {
                    let def = VariantDef {
                        doc: v.doc.clone(),
                        aliases: v.aliases.clone(),
                        deprecated: v.deprecated.clone(),
                    };
                    (v.name.clone(), def)
                })
                .collect(),
        };
        serde_json::to_value(def).unwrap_or(Value::Null)
    }

    fn field_def(&self, f: &FieldDesc) -> FieldDef {
        let named = match &f.ty {
            ValueType::Option(inner) => inner.as_ref(),
            other => other,
        };
        let options = match named {
            ValueType::Named(n) => self
                .get(n)
                .map(|t| t.variants().iter().map(|v| v.name.clone()).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        FieldDef {
            ty: f.ty.to_string(),
            doc: f.doc.clone(),
            default: (!f.required).then(|| self.field_default(f)),
            required: f.required,
            unit: f.unit,
            range: f.range.map(|r| (r.min, r.max)),
            options,
            examples: f.examples.clone(),
            since: f.since,
            deprecated: f.deprecated.clone(),
            aliases: f.aliases.clone(),
            replicate: f.replicate,
            editor: f.editor.clone(),
        }
    }

    /// Adds every type of a `*.types.json` document (`{ "$schema", "types": {...} }`).
    /// Types that fail to parse are skipped and reported; call [`verify`](Self::verify)
    /// afterwards to check cross-type references.
    pub fn add_json_types(&mut self, doc: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if doc.get("$schema").and_then(Value::as_str) != Some(TYPES_SCHEMA) {
            out.push(Diagnostic::error(
                codes::UNKNOWN_SCHEMA,
                format!("type files need `\"$schema\": \"{TYPES_SCHEMA}\"`"),
            ));
            return out;
        }
        let Some(types) = doc.get("types").and_then(Value::as_object) else {
            out.push(
                Diagnostic::error(codes::TYPE_MISMATCH, "expected a `types` object").path("types"),
            );
            return out;
        };
        for (name, def) in types {
            let path = format!("types[{name}]");
            match type_from_json(name, def) {
                Ok(desc) => {
                    if let Err(d) = self.add(desc) {
                        out.push(d.path(path));
                    }
                }
                Err(d) => out.push(d.path(path)),
            }
        }
        out
    }
}

fn kind_json(k: &DocKindDesc) -> Value {
    let mut m = Map::new();
    m.insert("doc".into(), k.doc.clone().into());
    m.insert("schema".into(), k.schema_uri().into());
    m.insert("idPrefix".into(), k.id_prefix.clone().into());
    m.insert("extension".into(), k.extension.clone().into());
    m.insert("records".into(), k.records.clone().into());
    if k.single_root {
        m.insert("singleRoot".into(), true.into());
    }
    Value::Object(m)
}

/// Parses one data-only type definition named `name`.
pub fn type_from_json(name: &str, v: &Value) -> Result<TypeDesc, Diagnostic> {
    let bad = |msg: String| Diagnostic::error(codes::INVALID_TYPE_DEF, format!("`{name}`: {msg}"));
    let def: TypeDef = serde_json::from_value(v.clone()).map_err(|e| bad(e.to_string()))?;
    let role = match def.kind.as_str() {
        "component" => StructRole::Component,
        "params" => StructRole::Params,
        "struct" => StructRole::Value,
        "record" => StructRole::Record(RecordInfo {
            prefix: def
                .prefix
                .clone()
                .ok_or_else(|| bad("records need a `prefix`".into()))?,
            components: def.components,
        }),
        "enum" => StructRole::Value,
        other => return Err(bad(format!("unknown kind `{other}`"))),
    };
    let shape = if def.kind == "enum" {
        let variants = def
            .variants
            .into_iter()
            .map(|(vname, v)| VariantDesc {
                name: vname,
                doc: v.doc,
                aliases: v.aliases,
                deprecated: v.deprecated,
            })
            .collect();
        Shape::Enum { variants }
    } else {
        let mut fields = Vec::new();
        for (fname, f) in def.fields {
            let ty: ValueType =
                f.ty.parse()
                    .map_err(|e: String| bad(format!("field `{fname}`: {e}")))?;
            fields.push(FieldDesc {
                name: fname,
                ty,
                doc: f.doc,
                default: f.default,
                required: f.required,
                unit: f.unit,
                range: f.range.map(|(min, max)| Range { min, max }),
                examples: f.examples,
                since: f.since,
                deprecated: f.deprecated,
                aliases: f.aliases,
                replicate: f.replicate,
                editor: f.editor,
            });
        }
        Shape::Struct { role, fields }
    };
    Ok(TypeDesc {
        name: name.to_owned(),
        doc: def.doc,
        module: def.module,
        version: def.version,
        aliases: def.aliases,
        deprecated: def.deprecated,
        shape,
        upgrade: None,
    })
}
