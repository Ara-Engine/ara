//! [`SchemaDb`]: registration, lookup and self-verification.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use ara_diag::{Diagnostic, codes, did_you_mean};
use serde_json::{Map, Value};

use crate::grammar::{is_field_name, is_id_prefix, is_kind_name, is_type_name};
use crate::{DocKindDesc, FieldDesc, Schema, Shape, StructRole, TypeDesc, Validator, ValueType};

/// The registry of every reflected type and document kind.
///
/// Fill it once at startup (Rust types via [`register`](Self::register), data-only
/// types via [`add`](Self::add) or [`add_json_types`](Self::add_json_types)), then
/// call [`verify`](Self::verify). Lookups are by stable name; old names resolve
/// through aliases.
#[derive(Debug, Clone, Default)]
pub struct SchemaDb {
    types: BTreeMap<String, TypeDesc>,
    aliases: BTreeMap<String, String>,
    kinds: BTreeMap<String, DocKindDesc>,
    rust: HashMap<String, TypeId>,
    problems: Vec<Diagnostic>,
}

impl SchemaDb {
    /// An empty DB.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a Rust type and the types it refers to. Registering the same type
    /// twice is a no-op; a clash with another type is reported by [`verify`](Self::verify).
    pub fn register<T: Schema>(&mut self) -> &mut Self {
        let desc = T::describe();
        let id = TypeId::of::<T>();
        if self.rust.get(&desc.name) == Some(&id) {
            return self;
        }
        let name = desc.name.clone();
        match self.add(desc) {
            Ok(()) => {
                self.rust.insert(name, id);
                T::register_deps(self);
            }
            Err(d) => self.problems.push(d),
        }
        self
    }

    /// Adds a type described at runtime (dynamic, data-only types use this).
    pub fn add(&mut self, desc: TypeDesc) -> Result<(), Diagnostic> {
        if let Some(owner) = self.claimant(&desc.name) {
            return Err(Diagnostic::error(
                codes::DUPLICATE_TYPE,
                format!("type `{}` is already registered (as `{owner}`)", desc.name),
            ));
        }
        for alias in &desc.aliases {
            if let Some(owner) = self.claimant(alias) {
                return Err(Diagnostic::error(
                    codes::DUPLICATE_TYPE,
                    format!(
                        "alias `{alias}` of `{}` is already used by `{owner}`",
                        desc.name
                    ),
                ));
            }
        }
        for alias in &desc.aliases {
            self.aliases.insert(alias.clone(), desc.name.clone());
        }
        self.types.insert(desc.name.clone(), desc);
        Ok(())
    }

    /// Removes a type (and its aliases) by current name, returning it. Hosts that
    /// register types at runtime (C# modules) use this to replace a type's definition
    /// when its module reloads.
    pub fn remove(&mut self, name: &str) -> Option<TypeDesc> {
        let desc = self.types.remove(name)?;
        self.aliases.retain(|_, owner| owner != name);
        self.rust.remove(name);
        Some(desc)
    }

    /// The current type that owns `name` as its name or an alias.
    fn claimant(&self, name: &str) -> Option<&str> {
        self.types
            .get_key_value(name)
            .map(|(k, _)| k.as_str())
            .or_else(|| self.aliases.get(name).map(String::as_str))
    }

    /// Adds a document kind.
    pub fn add_kind(&mut self, kind: DocKindDesc) -> Result<(), Diagnostic> {
        if self.kinds.contains_key(&kind.name) {
            return Err(Diagnostic::error(
                codes::DUPLICATE_TYPE,
                format!("document kind `{}` is already registered", kind.name),
            ));
        }
        self.kinds.insert(kind.name.clone(), kind);
        Ok(())
    }

    /// A type by its current name.
    pub fn get(&self, name: &str) -> Option<&TypeDesc> {
        self.types.get(name)
    }

    /// A type by current name or alias. The flag is true when `name` is an alias.
    pub fn lookup(&self, name: &str) -> Option<(&TypeDesc, bool)> {
        if let Some(t) = self.types.get(name) {
            return Some((t, false));
        }
        let current = self.aliases.get(name)?;
        self.types.get(current).map(|t| (t, true))
    }

    /// All types, sorted by name.
    pub fn types(&self) -> impl Iterator<Item = &TypeDesc> {
        self.types.values()
    }

    /// All component types, sorted by name.
    pub fn components(&self) -> impl Iterator<Item = &TypeDesc> {
        self.types.values().filter(|t| t.is_component())
    }

    /// A document kind by name.
    pub fn kind(&self, name: &str) -> Option<&DocKindDesc> {
        self.kinds.get(name)
    }

    /// All document kinds, sorted by name.
    pub fn kinds(&self) -> impl Iterator<Item = &DocKindDesc> {
        self.kinds.values()
    }

    /// Resolves a `$schema` URI (`ara://schema/scene/1`) to its kind and the version
    /// the document was written with.
    pub fn kind_for_schema(&self, uri: &str) -> Result<(&DocKindDesc, u32), Diagnostic> {
        let unknown =
            || Diagnostic::error(codes::UNKNOWN_SCHEMA, format!("unknown `$schema` `{uri}`"));
        let rest = uri.strip_prefix("ara://schema/").ok_or_else(unknown)?;
        let (name, version) = rest.split_once('/').ok_or_else(unknown)?;
        let version: u32 = version.parse().map_err(|_| unknown())?;
        let kind = self.kinds.get(name).ok_or_else(unknown)?;
        if version > kind.version {
            return Err(Diagnostic::error(
                codes::NEWER_VERSION,
                format!(
                    "`{uri}` is newer than this engine's `{}`",
                    kind.schema_uri()
                ),
            ));
        }
        Ok((kind, version))
    }

    /// The record type a document kind uses for IDs with `prefix`.
    pub fn record_type(&self, kind: &DocKindDesc, prefix: &str) -> Option<&TypeDesc> {
        kind.records
            .iter()
            .filter_map(|r| self.types.get(r))
            .find(|t| t.record_info().is_some_and(|i| i.prefix == prefix))
    }

    /// The zero value of a type: `0`, `""`, `false`, `[0, 0, 0]`, `{}` for structs,
    /// the first variant for enums, `null` for references and options.
    pub fn zero(&self, ty: &ValueType) -> Value {
        match ty {
            ValueType::Bool => Value::Bool(false),
            ValueType::String => Value::String(String::new()),
            ValueType::Vec2 => Value::from(vec![0; 2]),
            ValueType::Vec3 => Value::from(vec![0; 3]),
            ValueType::Vec4 => Value::from(vec![0; 4]),
            ValueType::Quat => Value::from(vec![0, 0, 0, 1]),
            ValueType::Color => Value::from(vec![1; 4]),
            ValueType::Entity | ValueType::Asset(_) | ValueType::Json | ValueType::Option(_) => {
                Value::Null
            }
            ValueType::List(_) | ValueType::Keyed(_) => Value::Array(Vec::new()),
            ValueType::Map(_) => Value::Object(Map::new()),
            ValueType::Named(n) => match self.types.get(n).map(|t| &t.shape) {
                Some(Shape::Enum { variants }) => variants
                    .first()
                    .map_or(Value::Null, |v| Value::from(v.name.clone())),
                _ => Value::Object(Map::new()),
            },
            _ => Value::from(0),
        }
    }

    /// The effective default of a field: its explicit default or its type's zero.
    pub fn field_default(&self, field: &FieldDesc) -> Value {
        field
            .default
            .clone()
            .unwrap_or_else(|| self.zero(&field.ty))
    }

    /// Checks every registered type and kind: names, required docs, resolvable field
    /// types, valid defaults and examples, unique record prefixes. Returns all problems,
    /// including registration clashes.
    pub fn verify(&self) -> Vec<Diagnostic> {
        let mut out = self.problems.clone();
        for t in self.types.values() {
            self.verify_type(t, &mut out);
        }
        let mut prefixes = BTreeMap::new();
        for t in self.types.values() {
            if let Some(info) = t.record_info()
                && let Some(other) = prefixes.insert(info.prefix.clone(), t.name.clone())
            {
                out.push(bad_def(
                    &t.name,
                    format!("record prefix `{}` is also used by `{other}`", info.prefix),
                ));
            }
        }
        for k in self.kinds.values() {
            if !is_kind_name(&k.name) || !is_id_prefix(&k.id_prefix) || k.doc.trim().is_empty() {
                out.push(bad_def(
                    &k.name,
                    "document kinds need a lowercase name, a 1-3 letter ID prefix and a doc",
                ));
            }
            for r in &k.records {
                if self.types.get(r).and_then(TypeDesc::record_info).is_none() {
                    out.push(bad_def(
                        &k.name,
                        format!("`{r}` is not a registered record type"),
                    ));
                }
            }
        }
        out
    }

    fn verify_type(&self, t: &TypeDesc, out: &mut Vec<Diagnostic>) {
        let mut problem = |msg: String| out.push(bad_def(&t.name, msg));
        if !is_type_name(&t.name) {
            problem(format!("type name `{}` must be PascalCase", t.name));
        }
        if t.doc.trim().is_empty() {
            problem("missing doc comment".to_owned());
        }
        if t.version == 0 {
            problem("versions start at 1".to_owned());
        }
        for alias in &t.aliases {
            if !is_type_name(alias) {
                problem(format!("alias `{alias}` must be PascalCase"));
            }
        }
        match &t.shape {
            Shape::Enum { variants } => {
                if variants.is_empty() {
                    problem("enums need at least one variant".to_owned());
                }
                let mut seen = BTreeSet::new();
                for v in variants {
                    if !is_field_name(&v.name) || v.doc.trim().is_empty() {
                        problem(format!(
                            "variant `{}` needs a camelCase name and a doc comment",
                            v.name
                        ));
                    }
                    for name in std::iter::once(&v.name).chain(&v.aliases) {
                        if !seen.insert(name.as_str()) {
                            problem(format!("variant name `{name}` is used twice"));
                        }
                    }
                }
            }
            Shape::Struct { role, fields } => {
                if let StructRole::Record(info) = role
                    && !is_id_prefix(&info.prefix)
                {
                    problem(format!(
                        "record prefix `{}` must be 1-3 lowercase letters",
                        info.prefix
                    ));
                }
                let mut seen = BTreeSet::new();
                for f in fields {
                    for name in std::iter::once(&f.name).chain(&f.aliases) {
                        if !seen.insert(name.as_str()) {
                            problem(format!("field name `{name}` is used twice"));
                        }
                    }
                    self.verify_field(t, role, f, &mut problem);
                }
            }
        }
    }

    fn verify_field(
        &self,
        t: &TypeDesc,
        role: &StructRole,
        f: &FieldDesc,
        problem: &mut impl FnMut(String),
    ) {
        let name = &f.name;
        let mut sound = true;
        let mut fail = |msg: String| {
            sound = false;
            problem(msg);
        };
        if !is_field_name(name) {
            fail(format!("field `{name}` must be camelCase"));
        }
        if f.doc.trim().is_empty() {
            fail(format!("field `{name}` is missing its doc comment"));
        }
        if f.required && *role != StructRole::Params {
            fail(format!(
                "field `{name}`: only params fields can be required"
            ));
        }
        for n in f.ty.named() {
            if !self.types.contains_key(n) {
                fail(format!("field `{name}` has unknown type `{n}`"));
            }
        }
        if let ValueType::Keyed(item) = &f.ty {
            let has_id = match item.as_ref() {
                ValueType::Named(n) => self
                    .types
                    .get(n)
                    .and_then(|i| i.field("id"))
                    .is_some_and(|id| id.ty == ValueType::String),
                _ => false,
            };
            if !has_id {
                fail(format!(
                    "field `{name}`: keyed items must be structs with an `id: string` field"
                ));
            }
        }
        let numeric = |ty: &ValueType| ty.is_number() || ty.vector_len().is_some();
        let base = match &f.ty {
            ValueType::Option(inner) => inner.as_ref(),
            other => other,
        };
        if (f.unit.is_some() || f.range.is_some()) && !numeric(base) {
            fail(format!(
                "field `{name}`: unit and range need a numeric type"
            ));
        }
        if let Some(r) = f.range
            && let (Some(lo), Some(hi)) = (r.min, r.max)
            && lo > hi
        {
            fail(format!("field `{name}`: empty range {r}"));
        }
        if !sound {
            // Defaults and examples can't be judged against a broken field.
            return;
        }
        let mut diags = Vec::new();
        let mut v = Validator::new(self, &mut diags);
        if !f.required {
            v.value(&f.ty, Some(f), &self.field_default(f), name);
        }
        for example in &f.examples {
            v.value(&f.ty, Some(f), example, name);
        }
        for d in diags.into_iter().filter(Diagnostic::is_error) {
            problem(format!(
                "default or example of `{}.{name}`: {}",
                t.name, d.message
            ));
        }
    }

    /// Suggests a registered type name close to `name`.
    pub fn suggest_type(&self, name: &str, filter: impl Fn(&TypeDesc) -> bool) -> Option<&str> {
        did_you_mean(
            name,
            self.types
                .values()
                .filter(|t| filter(t))
                .map(|t| t.name.as_str()),
        )
    }
}

fn bad_def(ty: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        codes::INVALID_TYPE_DEF,
        format!("`{ty}`: {}", message.into()),
    )
}
