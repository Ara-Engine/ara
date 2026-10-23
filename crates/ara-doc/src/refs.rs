//! `@path#id` references: the asset index and self-repair.
//!
//! The ID is authoritative and the path is for readers. When one half breaks, the
//! other repairs it: a stale path is rewritten from the ID, a missing ID is filled in
//! from the path. Only when neither resolves is the reference an error.

use std::collections::BTreeMap;

use ara_diag::{Diagnostic, FixAction, Location, codes, nearest};
use ara_schema::grammar::{format_ref, parse_ref};
use ara_schema::{FieldPath, SchemaDb, Shape, TypeDesc, ValueType};
use serde_json::Value;

use crate::document::{Record, field, is_component_key};

/// Every addressable asset in a project: ID ↔ project-relative path, plus the
/// document kind (`None` for imported source media with an `.import.json` sidecar).
#[derive(Debug, Clone, Default)]
pub struct AssetIndex {
    by_id: BTreeMap<String, (String, Option<String>)>,
    by_path: BTreeMap<String, String>,
}

impl AssetIndex {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `id` lives at `path`.
    pub fn insert(&mut self, id: impl Into<String>, path: impl Into<String>, kind: Option<String>) {
        let (id, path) = (id.into(), path.into());
        self.by_path.insert(path.clone(), id.clone());
        self.by_id.insert(id, (path, kind));
    }

    /// Path of an asset ID.
    pub fn path_of(&self, id: &str) -> Option<&str> {
        self.by_id.get(id).map(|(p, _)| p.as_str())
    }

    /// Asset ID at a path.
    pub fn id_of(&self, path: &str) -> Option<&str> {
        self.by_path.get(path).map(String::as_str)
    }

    /// Document kind of an asset, if it is a document.
    pub fn kind_of(&self, id: &str) -> Option<&str> {
        self.by_id.get(id).and_then(|(_, k)| k.as_deref())
    }

    /// The known assets closest to a reference that does not resolve (by path, else
    /// by ID), of `kind` when given, as full `@path#id`s; several when tied.
    pub fn closest(&self, text: &str, kind: Option<&str>) -> Vec<String> {
        let Ok(parts) = parse_ref(text) else {
            return Vec::new();
        };
        let fits = |id: &str| kind.is_none_or(|k| self.kind_of(id) == Some(k));
        let ids: Vec<&str> = match (parts.path, parts.id) {
            (Some(path), _) => {
                let paths = self
                    .by_path
                    .iter()
                    .filter(|(_, id)| fits(id))
                    .map(|(p, _)| p.as_str());
                nearest(path, paths)
                    .into_iter()
                    .filter_map(|p| self.id_of(p))
                    .collect()
            }
            (None, Some(id)) => nearest(
                id,
                self.by_id.keys().map(String::as_str).filter(|i| fits(i)),
            ),
            (None, None) => Vec::new(),
        };
        ids.into_iter()
            .filter_map(|id| Some(format_ref(self.path_of(id)?, id)))
            .collect()
    }

    /// Forgets an asset (its file was deleted).
    pub fn remove(&mut self, id: &str) {
        if let Some((path, _)) = self.by_id.remove(id) {
            self.by_path.remove(&path);
        }
    }
}

/// Resolves one reference. Returns the canonical text and, if it changed, a note
/// explaining the repair (no location; the caller adds it).
pub fn resolve(
    text: &str,
    kind: Option<&str>,
    index: &AssetIndex,
) -> Result<(String, Option<Diagnostic>), Diagnostic> {
    let parts = parse_ref(text).map_err(|msg| Diagnostic::error(codes::INVALID_REF, msg))?;
    let by_id = parts.id.and_then(|id| index.path_of(id).map(|p| (id, p)));
    let by_path = parts.path.and_then(|p| index.id_of(p).map(|id| (id, p)));
    let (id, path, note) = match (by_id, by_path, parts.path) {
        (Some((id, path)), _, given) if given == Some(path) => (id, path, None),
        (Some((id, path)), _, Some(stale)) => (
            id,
            path,
            Some(Diagnostic::warning(
                codes::REF_PATH_REPAIRED,
                format!("stale path `{stale}` in `{text}`; `{id}` now lives at `{path}`"),
            )),
        ),
        (Some((id, path)), _, None) => (
            id,
            path,
            Some(Diagnostic::info(
                codes::REF_FILLED,
                format!("filled in path `{path}` for `{text}`"),
            )),
        ),
        (None, Some((id, path)), _) => {
            let note = match parts.id {
                Some(old) => Diagnostic::warning(
                    codes::REF_PATH_REPAIRED,
                    format!("unknown ID `{old}` in `{text}`; `{path}` has ID `{id}`"),
                ),
                None => Diagnostic::info(
                    codes::REF_FILLED,
                    format!("filled in ID `{id}` for `{text}`"),
                ),
            };
            (id, path, Some(note))
        }
        (None, None, _) => {
            return Err(Diagnostic::error(
                codes::UNRESOLVED_REF,
                format!("`{text}` does not resolve: no asset has that ID or path"),
            ));
        }
    };
    if let Some(want) = kind
        && index.kind_of(id) != Some(want)
    {
        let got = index.kind_of(id).unwrap_or("source asset");
        return Err(Diagnostic::error(
            codes::REF_KIND_MISMATCH,
            format!("`{text}` is a {got}, expected a {want}"),
        ));
    }
    Ok((format_ref(path, id), note))
}

/// Repairs every reference in a record in place (fields, components and override
/// values), reporting repairs and failures at `base`.
pub fn repair_record(
    db: &SchemaDb,
    record_type: &TypeDesc,
    rec: &mut Record,
    index: &AssetIndex,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) {
    let mut walker = Walker {
        db,
        index,
        base,
        out,
    };
    for (key, v) in rec.iter_mut() {
        if is_component_key(key) {
            if db.get(key).is_some_and(TypeDesc::is_component) {
                walker.value(&ValueType::Named(key.clone()), v, key);
            }
        } else if key == field::OVERRIDES {
            let Some(map) = v.as_object_mut() else {
                continue;
            };
            for (okey, ov) in map.iter_mut() {
                let Some(ty) = override_type(db, record_type, okey) else {
                    continue;
                };
                walker.value(&ty, ov, &format!("{}[{okey}]", field::OVERRIDES));
            }
        } else if let Some(f) = record_type.field(key) {
            walker.value(&f.ty, v, key);
        }
    }
}

/// The schema type of an override value, from the field-path part of its key.
pub fn override_type(db: &SchemaDb, record_type: &TypeDesc, key: &str) -> Option<ValueType> {
    let path_part = key.rsplit_once('/').map_or(key, |(_, p)| p);
    let path: FieldPath = path_part.parse().ok()?;
    db.resolve_path(record_type, &path).ok().map(|r| r.ty)
}

struct Walker<'a> {
    db: &'a SchemaDb,
    index: &'a AssetIndex,
    base: &'a Location,
    out: &'a mut Vec<Diagnostic>,
}

impl Walker<'_> {
    fn value(&mut self, ty: &ValueType, v: &mut Value, path: &str) {
        match (ty, v) {
            (ValueType::Asset(kind), Value::String(s)) => {
                match resolve(s, kind.as_deref(), self.index) {
                    Ok((canonical, note)) => {
                        if let Some(note) = note {
                            let fix = FixAction::Set {
                                record: self.base.record.clone(),
                                path: path.to_owned(),
                                value: Value::from(canonical.clone()),
                            };
                            self.out.push(
                                note.path(path)
                                    .within(self.base)
                                    .with_fix(format!("Write `{canonical}`"), fix),
                            );
                        }
                        *s = canonical;
                    }
                    Err(mut d) => {
                        let near = if d.code == codes::UNRESOLVED_REF {
                            self.index.closest(s, kind.as_deref())
                        } else {
                            Vec::new()
                        };
                        if !near.is_empty() {
                            let list: Vec<String> = near.iter().map(|n| format!("`{n}`")).collect();
                            d.message
                                .push_str(&format!("; did you mean {}?", list.join(" or ")));
                        }
                        for n in near {
                            d = d.with_fix(
                                format!("Use `{n}`"),
                                FixAction::Set {
                                    record: self.base.record.clone(),
                                    path: path.to_owned(),
                                    value: Value::from(n),
                                },
                            );
                        }
                        self.out.push(d.path(path).within(self.base));
                    }
                }
            }
            (ValueType::Option(inner), v) => self.value(inner, v, path),
            (ValueType::List(item), Value::Array(items)) => {
                for (i, it) in items.iter_mut().enumerate() {
                    self.value(item, it, &format!("{path}[#{i}]"));
                }
            }
            (ValueType::Keyed(item), Value::Array(items)) => {
                for it in items.iter_mut() {
                    let id = it
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_owned();
                    self.value(item, it, &format!("{path}[{id}]"));
                }
            }
            (ValueType::Map(item), Value::Object(map)) => {
                for (k, it) in map.iter_mut() {
                    self.value(item, it, &format!("{path}[{k}]"));
                }
            }
            (ValueType::Named(n), Value::Object(obj)) => {
                let Some(t) = self.db.get(n) else { return };
                if !matches!(t.shape, Shape::Struct { .. }) {
                    return;
                }
                for (k, fv) in obj.iter_mut() {
                    if let Some(f) = t.field(k) {
                        self.value(&f.ty, fv, &format!("{path}.{k}"));
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> AssetIndex {
        let mut ix = AssetIndex::new();
        ix.insert("pf_rifle001", "prefabs/rifle.prefab", Some("prefab".into()));
        ix.insert("ms_crate001", "meshes/crate.glb", None);
        ix
    }

    #[test]
    fn either_half_repairs_the_other() {
        let ix = index();
        let ok = |s: &str| resolve(s, None, &ix).expect("resolves");

        assert_eq!(
            ok("@prefabs/rifle.prefab#pf_rifle001"),
            ("@prefabs/rifle.prefab#pf_rifle001".into(), None)
        );

        let (text, note) = ok("@old/rifle.prefab#pf_rifle001");
        assert_eq!(text, "@prefabs/rifle.prefab#pf_rifle001");
        assert_eq!(note.expect("repaired").code, codes::REF_PATH_REPAIRED);

        let (text, note) = ok("@prefabs/rifle.prefab");
        assert_eq!(text, "@prefabs/rifle.prefab#pf_rifle001");
        assert_eq!(note.expect("filled").code, codes::REF_FILLED);

        let (text, _) = ok("@#ms_crate001");
        assert_eq!(text, "@meshes/crate.glb#ms_crate001");

        let (text, note) = ok("@meshes/crate.glb#ms_gone0000");
        assert_eq!(text, "@meshes/crate.glb#ms_crate001");
        assert_eq!(note.expect("repaired").code, codes::REF_PATH_REPAIRED);
    }

    #[test]
    fn unresolvable_and_wrong_kind_fail() {
        let ix = index();
        assert_eq!(
            resolve("@nowhere.prefab#pf_nope0000", None, &ix)
                .expect_err("unresolved")
                .code,
            codes::UNRESOLVED_REF
        );
        assert_eq!(
            resolve("@meshes/crate.glb", Some("prefab"), &ix)
                .expect_err("wrong kind")
                .code,
            codes::REF_KIND_MISMATCH
        );
        assert_eq!(
            resolve("meshes/crate.glb", None, &ix)
                .expect_err("syntax")
                .code,
            codes::INVALID_REF
        );
    }

    #[test]
    fn broken_references_suggest_the_closest_asset_of_their_kind() {
        let ix = index();
        let near = |s: &str, k: Option<&str>| ix.closest(s, k);
        assert_eq!(
            near("@prefabs/rilfe.prefab#pf_gone0000", Some("prefab")),
            ["@prefabs/rifle.prefab#pf_rifle001"]
        );
        assert!(
            near("@meshes/crate.glb", Some("prefab")).is_empty(),
            "wrong kind"
        );
        assert_eq!(
            near("@#ms_crate002", None),
            ["@meshes/crate.glb#ms_crate001"]
        );
        assert!(near("@far/away.bin", None).is_empty());
    }
}
