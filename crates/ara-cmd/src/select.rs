//! Entity selectors in command params.
//!
//! Wherever a param is typed `entity`, a call may write:
//!
//! - an ID or address: `e_8b21`, `e_4f9a/e_m4tz` (inside a prefab instance);
//! - a batch symbol: `$crate`;
//! - a name selector: `name:Crate`, or a path of names ending at the entity
//!   (`name:Floor/Crate`, `name:DoorLamp/Bulb`), as long as it matches exactly one
//!   entity (prefab contents included).
//!
//! Name selectors are resolved to addresses before the params are validated, so
//! commands only ever see addresses. A bare name (`Crate`) is still an error, but
//! its diagnostic carries a machine-applicable fix per matching entity.
//!
//! Asset-typed params may leave out the `@`: `prefabs/crate.prefab` reads as
//! `@prefabs/crate.prefab`, and a bare asset ID `pf_crate001` as `@#pf_crate001`.

use ara_diag::{Diagnostic, FixAction, codes, did_you_mean};
use ara_doc::prefab::Resolved;
use ara_schema::grammar::{is_entity_address, split_id};
use ara_schema::{SchemaDb, ValueType};
use serde_json::Value;

/// The selector prefix.
pub const NAME_PREFIX: &str = "name:";

/// What an entity-typed string names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selector<'a> {
    /// An ID or entity address.
    Address(&'a str),
    /// A batch symbol (`$crate`).
    Symbol(&'a str),
    /// `name:A/B`: names from an ancestor down to the entity.
    Names(&'a str),
    /// Anything else, most often a bare name.
    Bare(&'a str),
}

impl<'a> Selector<'a> {
    /// Classifies an entity-typed string.
    pub fn parse(s: &'a str) -> Self {
        if let Some(path) = s.strip_prefix(NAME_PREFIX) {
            Selector::Names(path)
        } else if s.starts_with('$') {
            Selector::Symbol(s)
        } else if is_entity_address(s) {
            Selector::Address(s)
        } else {
            Selector::Bare(s)
        }
    }
}

fn name_of<'r>(entities: &'r Resolved, addr: &str) -> Option<&'r str> {
    entities.get(addr)?.record.get("name")?.as_str()
}

/// `name` (`Crate`) with its parent's name, for telling matches apart.
pub fn describe(entities: &Resolved, addr: &str) -> String {
    let name = name_of(entities, addr).unwrap_or("unnamed");
    let parent = entities
        .get(addr)
        .and_then(|e| e.parent.as_deref())
        .and_then(|p| name_of(entities, p));
    match parent {
        Some(p) => format!("{name}, under {p}"),
        None => name.to_owned(),
    }
}

/// True if the names from `addr` up its ancestors end with `path` (`A/B/C`).
fn matches_path(entities: &Resolved, addr: &str, path: &[&str], fold: bool) -> bool {
    let eq = |a: &str, b: &str| {
        if fold {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    let mut cur = Some(addr.to_owned());
    for want in path.iter().rev() {
        let Some(a) = cur else { return false };
        match entities.get(&a) {
            Some(e) if name_of(entities, &a).is_some_and(|n| eq(n, want)) => {
                cur = e.parent.clone();
            }
            _ => return false,
        }
    }
    true
}

/// Addresses of the entities a name path (`Floor/Crate`) matches: exact names first,
/// else ignoring ASCII case.
pub fn by_names(entities: &Resolved, path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').collect();
    for fold in [false, true] {
        let hits: Vec<String> = entities
            .keys()
            .filter(|a| matches_path(entities, a, &parts, fold))
            .cloned()
            .collect();
        if !hits.is_empty() {
            return hits;
        }
    }
    Vec::new()
}

/// Addresses a selector matches in `entities` (symbols match nothing here).
pub fn find(entities: &Resolved, sel: Selector<'_>) -> Vec<String> {
    match sel {
        Selector::Address(a) => entities
            .contains_key(a)
            .then(|| a.to_owned())
            .into_iter()
            .collect(),
        Selector::Names(path) | Selector::Bare(path) => by_names(entities, path),
        Selector::Symbol(_) => Vec::new(),
    }
}

fn use_fix(entities: &Resolved, d: Diagnostic, path: &str, hits: &[String]) -> Diagnostic {
    hits.iter().fold(d, |d, addr| {
        d.with_fix(
            format!("Use `{addr}` ({})", describe(entities, addr)),
            FixAction::Set {
                record: None,
                path: path.to_owned(),
                value: addr.clone().into(),
            },
        )
    })
}

/// Resolves one entity-typed string in place.
fn resolve_one(entities: &Resolved, s: &mut String, path: &str) -> Result<(), Diagnostic> {
    match Selector::parse(s) {
        Selector::Address(_) | Selector::Symbol(_) => Ok(()),
        Selector::Names(names) => {
            let hits = by_names(entities, names);
            match hits.as_slice() {
                [one] => {
                    *s = one.clone();
                    Ok(())
                }
                [] => {
                    let all: Vec<&str> = entities.keys().filter_map(|a| name_of(entities, a)).collect();
                    let last = names.rsplit('/').next().unwrap_or(names);
                    let near = did_you_mean(last, all.iter().copied());
                    let mut d = Diagnostic::error(
                        codes::RECORD_NOT_FOUND,
                        format!("no entity matches `{s}` in this document"),
                    )
                    .path(path);
                    if let Some(n) = near {
                        d.message.push_str(&format!("; did you mean `name:{n}`?"));
                        let close = by_names(entities, n);
                        d = use_fix(entities, d, path, &close);
                    }
                    Err(d)
                }
                many => Err(use_fix(
                    entities,
                    Diagnostic::error(
                        codes::AMBIGUOUS_ENTITY,
                        format!(
                            "`{s}` matches {} entities; use one of their IDs, or name a parent too (`name:Parent/Child`)",
                            many.len()
                        ),
                    )
                    .path(path),
                    path,
                    many,
                )),
            }
        }
        Selector::Bare(bare) => {
            let mut hits = by_names(entities, bare);
            let mut message = format!(
                "expected an entity ID like `e_8b21` (or `name:{bare}`), got the name `{bare}`"
            );
            if hits.is_empty() {
                let all: Vec<&str> = entities
                    .keys()
                    .filter_map(|a| name_of(entities, a))
                    .collect();
                match did_you_mean(bare, all.iter().copied()) {
                    Some(n) => hits = by_names(entities, n),
                    None => {
                        message = format!(
                            "expected an entity ID like `e_8b21` or a selector like `name:Crate`, got `{bare}`"
                        );
                    }
                }
            }
            if let [one] = hits.as_slice() {
                message.push_str(&format!(
                    "; did you mean `{one}` ({})?",
                    describe(entities, one)
                ));
            }
            Err(use_fix(
                entities,
                Diagnostic::error(codes::TYPE_MISMATCH, message).path(path),
                path,
                &hits,
            ))
        }
    }
}

/// Visits every entity- and asset-typed string of `v` (of type `ty`) with its
/// type and display path.
fn walk(
    db: &SchemaDb,
    ty: &ValueType,
    v: &mut Value,
    path: &str,
    f: &mut Visit<'_>,
) -> Result<(), Diagnostic> {
    match (ty, v) {
        (ValueType::Entity | ValueType::Asset(_), Value::String(s)) => f(ty, s, path),
        (ValueType::Option(inner), v) if !v.is_null() => walk(db, inner, v, path, f),
        (ValueType::List(item) | ValueType::Keyed(item), Value::Array(items)) => {
            for (i, it) in items.iter_mut().enumerate() {
                walk(db, item, it, &format!("{path}[#{i}]"), f)?;
            }
            Ok(())
        }
        (ValueType::Map(item), Value::Object(o)) => {
            for (k, it) in o.iter_mut() {
                walk(db, item, it, &format!("{path}[{k}]"), f)?;
            }
            Ok(())
        }
        (ValueType::Named(n), Value::Object(o)) => match db.get(n) {
            Some(t) => fields(db, t.fields(), o, path, f),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// What [`walk`] calls per leaf: the type, the string and its display path.
type Visit<'a> = dyn FnMut(&ValueType, &mut String, &str) -> Result<(), Diagnostic> + 'a;

fn fields(
    db: &SchemaDb,
    descs: &[ara_schema::FieldDesc],
    o: &mut serde_json::Map<String, Value>,
    path: &str,
    f: &mut Visit<'_>,
) -> Result<(), Diagnostic> {
    for field in descs {
        if let Some(v) = o.get_mut(&field.name) {
            walk(db, &field.ty, v, &format!("{path}.{}", field.name), f)?;
        }
    }
    Ok(())
}

/// Rewrites the name selectors in `params` (of params type `ty`) to addresses of
/// `entities` (bare names fail with a fix per match) and adds a missing `@` to
/// asset references.
pub fn resolve(
    db: &SchemaDb,
    ty: &str,
    params: &mut Value,
    entities: &Resolved,
) -> Result<(), Diagnostic> {
    let (Some(t), Value::Object(o)) = (db.get(ty), params) else {
        return Ok(());
    };
    fields(db, t.fields(), o, "params", &mut |ty, s, path| {
        if matches!(ty, ValueType::Entity) {
            return resolve_one(entities, s, path);
        }
        if !s.is_empty() && !s.starts_with('@') {
            *s = if split_id(s).is_some() {
                format!("@#{s}")
            } else {
                format!("@{s}")
            };
        }
        Ok(())
    })
}

/// Every entity-typed string in `params` (of params type `ty`), in order.
pub fn references(db: &SchemaDb, ty: &str, params: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut copy = params.clone();
    if let (Some(t), Value::Object(o)) = (db.get(ty), &mut copy) {
        let _ = fields(db, t.fields(), o, "params", &mut |ty, s, _| {
            if matches!(ty, ValueType::Entity) {
                out.push(s.clone());
            }
            Ok(())
        });
    }
    out
}
