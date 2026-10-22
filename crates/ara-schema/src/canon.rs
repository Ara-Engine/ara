//! Canonical form (reflection order, defaults omitted) and normalization of stored
//! data (renamed types and fields, version upgrades).

use std::collections::BTreeMap;

use ara_diag::{Diagnostic, codes};
use ara_json::{json_eq, sort_keys};
use serde_json::{Map, Value};

use crate::{SchemaDb, Shape, TypeDesc, ValueType};

impl SchemaDb {
    /// The canonical form of a value of type `ty`: struct keys in reflection order with
    /// defaults omitted, map keys sorted, recursively. Keys the schema doesn't know are
    /// kept, sorted, after the known ones, so no data is ever dropped.
    pub fn canonical_value(&self, ty: &ValueType, v: &Value) -> Value {
        match (ty, v) {
            (_, Value::Null) => Value::Null,
            (ValueType::Option(inner), _) => self.canonical_value(inner, v),
            (ValueType::Named(n), Value::Object(o)) => match self.get(n) {
                Some(t) if matches!(t.shape, Shape::Struct { .. }) => {
                    Value::Object(self.canonical_struct(t, o))
                }
                _ => sort_keys(v),
            },
            (ValueType::List(item) | ValueType::Keyed(item), Value::Array(items)) => Value::Array(
                items
                    .iter()
                    .map(|it| self.canonical_value(item, it))
                    .collect(),
            ),
            (ValueType::Map(item), Value::Object(o)) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|k| (k.clone(), self.canonical_value(item, &o[k])))
                        .collect(),
                )
            }
            _ => sort_keys(v),
        }
    }

    /// The canonical form of a struct value (see [`canonical_value`](Self::canonical_value)).
    pub fn canonical_struct(&self, ty: &TypeDesc, obj: &Map<String, Value>) -> Map<String, Value> {
        let mut out = Map::new();
        for f in ty.fields() {
            let Some(v) = obj.get(&f.name) else { continue };
            let cv = self.canonical_value(&f.ty, v);
            let default = self.canonical_value(&f.ty, &self.field_default(f));
            if f.required || !json_eq(&cv, &default) {
                out.insert(f.name.clone(), cv);
            }
        }
        let mut extra: Vec<&String> = obj.keys().filter(|k| ty.field(k).is_none()).collect();
        extra.sort();
        for k in extra {
            out.insert(k.clone(), sort_keys(&obj[k]));
        }
        out
    }

    /// Normalizes a stored component value in place: runs upgrade hooks from
    /// `stored_version` to the current version and renames aliased fields and enum
    /// variants. Reports each change; returns `false` if an upgrade failed.
    pub fn normalize_component(
        &self,
        ty: &TypeDesc,
        stored_version: u32,
        v: &mut Value,
        path: &str,
        out: &mut Vec<Diagnostic>,
    ) -> bool {
        if stored_version > ty.version {
            out.push(
                Diagnostic::error(
                    codes::NEWER_VERSION,
                    format!(
                        "`{}` was written as version {stored_version}; this engine has version {}",
                        ty.name, ty.version
                    ),
                )
                .path(path),
            );
            return false;
        }
        for from in stored_version..ty.version {
            if let Some(upgrade) = ty.upgrade
                && let Err(msg) = upgrade(from, v)
            {
                out.push(
                    Diagnostic::error(
                        codes::UPGRADE_FAILED,
                        format!("upgrading `{}` from version {from}: {msg}", ty.name),
                    )
                    .path(path),
                );
                return false;
            }
        }
        if stored_version < ty.version {
            out.push(
                Diagnostic::info(
                    codes::UPGRADED,
                    format!(
                        "upgraded `{}` from version {stored_version} to {}",
                        ty.name, ty.version
                    ),
                )
                .path(path),
            );
        }
        self.normalize_value(&ValueType::Named(ty.name.clone()), v, path, out);
        true
    }

    /// Renames aliased fields and enum variants inside `v`, recursively.
    pub fn normalize_value(
        &self,
        ty: &ValueType,
        v: &mut Value,
        path: &str,
        out: &mut Vec<Diagnostic>,
    ) {
        match (ty, v) {
            (ValueType::Option(inner), v) => self.normalize_value(inner, v, path, out),
            (ValueType::List(item) | ValueType::Keyed(item), Value::Array(items)) => {
                for it in items {
                    self.normalize_value(item, it, path, out);
                }
            }
            (ValueType::Map(item), Value::Object(o)) => {
                for it in o.values_mut() {
                    self.normalize_value(item, it, path, out);
                }
            }
            (ValueType::Named(n), v) => {
                let Some(t) = self.get(n) else { return };
                match (&t.shape, v) {
                    (Shape::Struct { .. }, Value::Object(o)) => {
                        self.normalize_fields(t, o, path, out)
                    }
                    (Shape::Enum { variants }, Value::String(s)) => {
                        if let Some(var) = variants.iter().find(|var| var.aliases.contains(s)) {
                            out.push(renamed(&format!("`{n}.{s}`"), &var.name, path));
                            *s = var.name.clone();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Renames aliased fields of a struct value and normalizes each field.
    pub fn normalize_fields(
        &self,
        ty: &TypeDesc,
        obj: &mut Map<String, Value>,
        path: &str,
        out: &mut Vec<Diagnostic>,
    ) {
        for f in ty.fields() {
            for alias in &f.aliases {
                if obj.contains_key(alias) && !obj.contains_key(&f.name) {
                    let v = obj.shift_remove(alias).unwrap_or(Value::Null);
                    out.push(renamed(
                        &format!("field `{alias}`"),
                        &f.name,
                        &join(path, alias),
                    ));
                    obj.insert(f.name.clone(), v);
                }
            }
            if let Some(v) = obj.get_mut(&f.name) {
                self.normalize_value(&f.ty, v, &join(path, &f.name), out);
            }
        }
    }

    /// Normalizes a record in place: renames aliased component keys, upgrades components
    /// from the document's `versions` table and renames aliased fields.
    pub fn normalize_record(
        &self,
        ty: &TypeDesc,
        obj: &mut Map<String, Value>,
        versions: &BTreeMap<String, u32>,
        out: &mut Vec<Diagnostic>,
    ) {
        let comps = ty.record_info().is_some_and(|i| i.components);
        if comps {
            let keys: Vec<String> = obj
                .keys()
                .filter(|k| k.starts_with(|c: char| c.is_ascii_uppercase()))
                .cloned()
                .collect();
            for key in keys {
                let Some((comp, via_alias)) = self.lookup(&key) else {
                    continue;
                };
                if !comp.is_component() {
                    continue;
                }
                let name = comp.name.clone();
                if via_alias {
                    if obj.contains_key(&name) {
                        continue;
                    }
                    out.push(renamed(&format!("component `{key}`"), &name, &key));
                }
                let mut v = obj.shift_remove(&key).unwrap_or(Value::Null);
                let stored = versions.get(&key).copied().unwrap_or(1);
                self.normalize_component(comp, stored, &mut v, &name, out);
                obj.insert(name, v);
            }
        }
        self.normalize_fields(ty, obj, "", out);
    }
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

fn renamed(what: &str, to: &str, path: &str) -> Diagnostic {
    Diagnostic::warning(
        codes::RENAMED,
        format!("{what} was renamed to `{to}`; `ara fmt` rewrites it"),
    )
    .path(path)
}
