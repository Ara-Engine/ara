//! Value validation against the schema, producing structured diagnostics.

use std::collections::BTreeSet;

use ara_diag::{Diagnostic, FixAction, Location, codes, did_you_mean};
use serde_json::{Map, Value};

use crate::grammar::{is_entity_address, parse_ref, split_id};
use crate::{FieldDesc, SchemaDb, Shape, TypeDesc, ValueType};

/// Validates values and records, appending diagnostics to `out`.
///
/// Paths in diagnostics are relative to whatever was passed in; `base` fills the rest
/// of the location (file, record).
pub struct Validator<'a> {
    db: &'a SchemaDb,
    out: &'a mut Vec<Diagnostic>,
    base: Location,
    symbols: bool,
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

fn key_path(path: &str, key: &str) -> String {
    format!("{path}[{key}]")
}

impl<'a> Validator<'a> {
    /// A validator writing into `out`.
    pub fn new(db: &'a SchemaDb, out: &'a mut Vec<Diagnostic>) -> Self {
        Validator {
            db,
            out,
            base: Location::default(),
            symbols: false,
        }
    }

    /// Accepts batch symbols (`$c1`) wherever an entity is expected. Command params use
    /// this; the command reports symbols that are still unbound when it resolves them.
    pub fn allow_symbols(mut self) -> Self {
        self.symbols = true;
        self
    }

    /// Sets the base location merged into every diagnostic.
    pub fn at(mut self, base: Location) -> Self {
        self.base = base;
        self
    }

    fn push(&mut self, d: Diagnostic) {
        self.out.push(d.within(&self.base));
    }

    fn mismatch(&mut self, path: &str, expected: impl std::fmt::Display, v: &Value) {
        let got = match v {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        };
        self.push(
            Diagnostic::error(
                codes::TYPE_MISMATCH,
                format!("expected {expected}, got {got}"),
            )
            .path(path),
        );
    }

    /// Validates a record of type `record`: its fields and, if the record carries
    /// components, every PascalCase key as a registered component.
    pub fn record(&mut self, record: &TypeDesc, obj: &Map<String, Value>) {
        let comps = record.record_info().is_some_and(|i| i.components);
        let mut fields = Map::new();
        for (key, v) in obj {
            if comps && key.starts_with(|c: char| c.is_ascii_uppercase()) {
                self.component(key, v);
            } else {
                fields.insert(key.clone(), v.clone());
            }
        }
        self.fields(record, &fields, "");
    }

    /// Validates one component value stored under `name`.
    pub fn component(&mut self, name: &str, v: &Value) {
        match self.db.get(name) {
            Some(t) if t.is_component() => {
                if let Some(msg) = &t.deprecated {
                    self.push(
                        Diagnostic::warning(
                            codes::DEPRECATED,
                            format!("component `{name}` is deprecated: {msg}"),
                        )
                        .path(name),
                    );
                }
                match v.as_object() {
                    Some(o) => self.fields(t, o, name),
                    None => self.mismatch(name, format!("a `{name}` object"), v),
                }
            }
            Some(t) => self.push(
                Diagnostic::error(
                    codes::NOT_A_COMPONENT,
                    format!("`{name}` is a {}, not a component", t.kind_name()),
                )
                .path(name),
            ),
            None => {
                let mut d =
                    Diagnostic::error(codes::UNKNOWN_TYPE, format!("unknown component `{name}`"))
                        .path(name);
                let suggestion = match self.db.lookup(name) {
                    Some((t, true)) => Some(t.name.clone()),
                    _ => self
                        .db
                        .suggest_type(name, TypeDesc::is_component)
                        .map(str::to_owned),
                };
                if let Some(s) = suggestion {
                    d = d.with_fix(
                        format!("Rename `{name}` to `{s}`"),
                        FixAction::Rename {
                            record: None,
                            path: name.to_owned(),
                            to: s,
                        },
                    );
                }
                self.push(d);
            }
        }
    }

    /// Validates the fields of a struct value: unknown keys, required fields and each
    /// field's value.
    pub fn fields(&mut self, ty: &TypeDesc, obj: &Map<String, Value>, path: &str) {
        for (key, v) in obj {
            match ty.field(key) {
                Some(f) => self.value(&f.ty, Some(f), v, &join(path, key)),
                None => self.unknown_field(ty, key, path),
            }
        }
        for f in ty
            .fields()
            .iter()
            .filter(|f| f.required && !obj.contains_key(&f.name))
        {
            self.push(
                Diagnostic::error(
                    codes::MISSING_REQUIRED,
                    format!("missing required field `{}` of `{}`", f.name, ty.name),
                )
                .path(join(path, &f.name)),
            );
        }
    }

    fn unknown_field(&mut self, ty: &TypeDesc, key: &str, path: &str) {
        let full = join(path, key);
        let renamed = ty
            .fields()
            .iter()
            .find(|f| f.aliases.iter().any(|a| a == key));
        let suggestion = renamed
            .map(|f| f.name.as_str())
            .or_else(|| did_you_mean(key, ty.fields().iter().map(|f| f.name.as_str())));
        let mut msg = format!("`{}` has no field `{key}`", ty.name);
        if renamed.is_some() {
            msg = format!("field `{key}` of `{}` was renamed", ty.name);
        }
        let mut d = Diagnostic::error(codes::UNKNOWN_FIELD, msg).path(full.clone());
        if let Some(s) = suggestion {
            d = d.with_fix(
                format!("Rename `{key}` to `{s}`"),
                FixAction::Rename {
                    record: None,
                    path: full,
                    to: s.to_owned(),
                },
            );
        }
        self.push(d);
    }

    /// Validates `v` against `ty`. `field` supplies range and deprecation metadata.
    pub fn value(&mut self, ty: &ValueType, field: Option<&FieldDesc>, v: &Value, path: &str) {
        if let Some(f) = field
            && let Some(msg) = &f.deprecated
        {
            self.push(
                Diagnostic::warning(
                    codes::DEPRECATED,
                    format!("field `{}` is deprecated: {msg}", f.name),
                )
                .path(path),
            );
        }
        let range = field.and_then(|f| f.range);
        match ty {
            ValueType::Bool if !v.is_boolean() => self.mismatch(path, "a boolean", v),
            ValueType::String if !v.is_string() => self.mismatch(path, "a string", v),
            t if t.is_number() => self.number(t, v, range, path),
            t if t.vector_len().is_some() => self.vector(t, v, range, path),
            ValueType::Entity => match v {
                Value::Null => {}
                Value::String(s) if is_entity_address(s) => {}
                Value::String(s) if self.symbols && s.starts_with('$') => {}
                _ => self.mismatch(path, "an entity ID like `e_8b21`", v),
            },
            ValueType::Asset(_) => match v {
                Value::Null => {}
                Value::String(s) => {
                    if let Err(msg) = parse_ref(s) {
                        self.push(Diagnostic::error(codes::INVALID_REF, msg).path(path));
                    }
                }
                _ => self.mismatch(path, "an `@path#id` reference", v),
            },
            ValueType::Option(inner) => {
                if !v.is_null() {
                    self.value(inner, field, v, path);
                }
            }
            ValueType::List(item) => match v.as_array() {
                Some(items) => {
                    for (i, it) in items.iter().enumerate() {
                        self.value(item, None, it, &format!("{path}[#{i}]"));
                    }
                }
                None => self.mismatch(path, "an array", v),
            },
            ValueType::Keyed(item) => self.keyed(item, v, path),
            ValueType::Map(item) => match v.as_object() {
                Some(o) => {
                    for (k, it) in o {
                        self.value(item, None, it, &key_path(path, k));
                    }
                }
                None => self.mismatch(path, "an object", v),
            },
            ValueType::Named(name) => self.named(name, v, path),
            _ => {}
        }
    }

    fn number(&mut self, ty: &ValueType, v: &Value, range: Option<crate::Range>, path: &str) {
        let Some(n) = v.as_number() else {
            return self.mismatch(path, format!("a number (`{ty}`)"), v);
        };
        if let Some((lo, hi)) = ty.int_bounds() {
            let int = n
                .as_i64()
                .map(i128::from)
                .or_else(|| n.as_u64().map(i128::from))
                .or_else(|| {
                    let f = n.as_f64()?;
                    // Integral floats (`2.0`) are accepted for integer fields.
                    (f.fract() == 0.0 && f.abs() < 1e19).then_some(f as i128)
                });
            match int {
                Some(i) if (lo..=hi).contains(&i) => {}
                Some(_) => {
                    return self.push(
                        Diagnostic::error(
                            codes::OUT_OF_RANGE,
                            format!("{n} does not fit in `{ty}`"),
                        )
                        .path(path),
                    );
                }
                None => return self.mismatch(path, format!("an integer (`{ty}`)"), v),
            }
        }
        if let Some(r) = range
            && let Some(f) = n.as_f64()
            && !r.contains(f)
        {
            self.push(
                Diagnostic::error(codes::OUT_OF_RANGE, format!("{n} is outside the range {r}"))
                    .path(path),
            );
        }
    }

    fn vector(&mut self, ty: &ValueType, v: &Value, range: Option<crate::Range>, path: &str) {
        let len = ty.vector_len().unwrap_or(0);
        match v.as_array() {
            Some(items) if items.len() == len && items.iter().all(Value::is_number) => {
                if let Some(r) = range {
                    for (i, it) in items.iter().enumerate() {
                        if let Some(f) = it.as_f64()
                            && !r.contains(f)
                        {
                            self.push(
                                Diagnostic::error(
                                    codes::OUT_OF_RANGE,
                                    format!("component {i} ({it}) is outside the range {r}"),
                                )
                                .path(path),
                            );
                        }
                    }
                }
            }
            _ => self.mismatch(path, format!("`{ty}`: an array of {len} numbers"), v),
        }
    }

    fn keyed(&mut self, item: &ValueType, v: &Value, path: &str) {
        let Some(items) = v.as_array() else {
            return self.mismatch(path, "an array of items with `id`", v);
        };
        let mut seen = BTreeSet::new();
        for it in items {
            match it.get("id").and_then(Value::as_str) {
                Some(id) if split_id(id).is_some() && seen.insert(id) => {
                    self.value(item, None, it, &key_path(path, id))
                }
                Some(id) if split_id(id).is_some() => {
                    self.push(
                        Diagnostic::error(
                            codes::INVALID_KEYED_ITEM,
                            format!("item id `{id}` is used twice"),
                        )
                        .path(key_path(path, id)),
                    );
                }
                _ => self.push(
                    Diagnostic::error(
                        codes::INVALID_KEYED_ITEM,
                        "keyed items need an `id` like `s_7c`",
                    )
                    .path(path),
                ),
            }
        }
    }

    fn named(&mut self, name: &str, v: &Value, path: &str) {
        let Some(t) = self.db.get(name) else {
            return self.push(
                Diagnostic::error(codes::UNKNOWN_TYPE, format!("unknown type `{name}`")).path(path),
            );
        };
        match &t.shape {
            Shape::Struct { .. } => match v.as_object() {
                Some(o) => self.fields(t, o, path),
                None => self.mismatch(path, format!("a `{name}` object"), v),
            },
            Shape::Enum { variants } => {
                let Some(s) = v.as_str() else {
                    return self.mismatch(path, format!("a `{name}` variant name"), v);
                };
                match variants.iter().find(|var| var.name == s) {
                    Some(var) => {
                        if let Some(msg) = &var.deprecated {
                            self.push(
                                Diagnostic::warning(
                                    codes::DEPRECATED,
                                    format!("`{name}.{s}` is deprecated: {msg}"),
                                )
                                .path(path),
                            );
                        }
                    }
                    None => {
                        let renamed = variants
                            .iter()
                            .find(|var| var.aliases.iter().any(|a| a == s));
                        let suggestion = renamed.map(|var| var.name.as_str()).or_else(|| {
                            did_you_mean(s, variants.iter().map(|var| var.name.as_str()))
                        });
                        let options: Vec<&str> =
                            variants.iter().map(|var| var.name.as_str()).collect();
                        let mut d = Diagnostic::error(
                            codes::UNKNOWN_VARIANT,
                            format!(
                                "`{s}` is not a `{name}`; expected one of: {}",
                                options.join(", ")
                            ),
                        )
                        .path(path);
                        if let Some(sug) = suggestion {
                            d = d.with_fix(
                                format!("Use `{sug}`"),
                                FixAction::Set {
                                    record: None,
                                    path: path.to_owned(),
                                    value: Value::from(sug),
                                },
                            );
                        }
                        self.push(d);
                    }
                }
            }
        }
    }
}
