//! Prefab instances: flattening a document into resolved entities.
//!
//! An instance record (`"prefab": "@prefabs/crate.prefab#pf_..."`) stands for the
//! prefab's root. Prefab entities appear under entity addresses: the instance's ID
//! joined with their ID inside the prefab (`e_4f9a/m4tz`), chaining through nested
//! instances (`e_4f9a/x1/k2`). The instance then applies, in order:
//! 1. `removed`: entity addresses (`m4tz`, with descendants) or components
//!    (`m4tz/Light`);
//! 2. `overrides`: `[<id path>/]<field path>` → value, in key order, so a whole
//!    component (`m4tz/Light`) lands before its fields (`m4tz/Light.color`).
//!
//! Only `name` and components can be overridden. Records of the outer document may
//! parent themselves inside an instance (`"parent": "e_4f9a/m4tz"`).

mod references;

use std::collections::BTreeMap;

use ara_diag::{Diagnostic, FixAction, Location, codes};
use ara_schema::grammar::parse_ref;
use ara_schema::{FieldPath, SchemaDb, Validator, path};
use serde_json::Value;

use crate::document::{Document, Record, field, is_component_key, order_of, parent_of, prefab_of};
use crate::{AssetId, RecordId};

/// Finds loaded documents by asset ID.
pub trait DocLookup {
    /// The document with this asset ID.
    fn lookup(&self, id: &str) -> Option<&Document>;
}

/// One entity after prefab resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEntity {
    /// `name` and components, with overrides applied.
    pub record: Record,
    /// Parent entity address.
    pub parent: Option<String>,
    /// Sibling order key.
    pub order: Option<String>,
    /// Where the entity is defined: document and record.
    pub source: (AssetId, RecordId),
}

/// Every entity of a document, keyed by entity address.
pub type Resolved = BTreeMap<String, ResolvedEntity>;

/// Resolves `doc`, reporting problems in its own instances (bad overrides, missing
/// prefabs). Problems inside the prefabs themselves are reported when those are checked.
pub fn resolve(
    doc: &Document,
    db: &SchemaDb,
    lookup: &dyn DocLookup,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) -> Resolved {
    let mut stack = vec![doc.id.clone()];
    let mut ctx = Ctx {
        db,
        lookup,
        stack: &mut stack,
    };
    let resolved = ctx.doc(doc, base, Some(out));
    check_parents(doc, &resolved, base, out);
    resolved
}

/// The check [`resolve`] runs across records once every record is resolved: a
/// record of `doc` whose parent is an address inside an instance that does not
/// exist. Reports what `resolve` reports, in the same order.
pub fn check_parents(
    doc: &Document,
    resolved: &Resolved,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) {
    for (addr, e) in resolved {
        // Plain parents are checked with the document's structure; only addresses
        // inside instances need the prefabs.
        if let Some(p) = &e.parent
            && p.contains('/')
            && !resolved.contains_key(p)
            && e.source.0 == doc.id
            && !addr.contains('/')
        {
            out.push(
                Diagnostic::error(
                    codes::MISSING_PARENT,
                    format!("parent `{p}` does not exist after prefab resolution"),
                )
                .record(addr.as_str())
                .path(field::PARENT)
                .within(base),
            );
        }
    }
}

/// The entities record `id` of `doc` contributes to [`resolve`], exactly as it has
/// them: a plain record itself, an instance's root at `id` and its prefab's other
/// entities at `id/…`. It costs that record alone, not the whole document. `None`
/// when the document has no such record.
pub fn resolve_record(
    doc: &Document,
    id: &RecordId,
    db: &SchemaDb,
    lookup: &dyn DocLookup,
) -> Option<Resolved> {
    resolve_record_reporting(doc, id, db, lookup, &Location::default(), None)
}

/// [`resolve_record`], also reporting the problems [`resolve`] reports while it
/// resolves this record (an instance's bad overrides or missing prefab), in the same
/// order. With [`check_parents`] afterwards, resolving every record this way
/// reports exactly what `resolve` does.
pub fn resolve_record_reporting(
    doc: &Document,
    id: &RecordId,
    db: &SchemaDb,
    lookup: &dyn DocLookup,
    base: &Location,
    out: Option<&mut Vec<Diagnostic>>,
) -> Option<Resolved> {
    let rec = doc.record(id.as_str())?;
    let mut stack = vec![doc.id.clone()];
    let mut ctx = Ctx {
        db,
        lookup,
        stack: &mut stack,
    };
    Some(ctx.record(doc, id, rec, base, out))
}

/// Reports the problems of one instance record of `doc` (missing or cyclic prefab,
/// bad overrides or removals) exactly as [`resolve`] reports them for it. A plain or
/// missing record reports nothing.
pub fn check_instance(
    doc: &Document,
    id: &RecordId,
    db: &SchemaDb,
    lookup: &dyn DocLookup,
    base: &Location,
    out: &mut Vec<Diagnostic>,
) {
    let Some(rec) = doc.record(id.as_str()) else {
        return;
    };
    let Some(reference) = prefab_of(rec) else {
        return;
    };
    let mut stack = vec![doc.id.clone()];
    let mut ctx = Ctx {
        db,
        lookup,
        stack: &mut stack,
    };
    let rbase = Location {
        record: Some(id.to_string()),
        ..base.clone()
    };
    ctx.instance(id, rec, reference, &rbase, Some(out));
}

struct Ctx<'a, 's> {
    db: &'a SchemaDb,
    lookup: &'a dyn DocLookup,
    stack: &'s mut Vec<AssetId>,
}

impl Ctx<'_, '_> {
    fn doc(
        &mut self,
        doc: &Document,
        base: &Location,
        mut out: Option<&mut Vec<Diagnostic>>,
    ) -> Resolved {
        let mut all = Resolved::new();
        for (id, rec) in doc.records() {
            all.extend(self.record(doc, id, rec, base, out.as_deref_mut()));
        }
        all
    }

    /// One record's entities: itself when plain, else its instance expanded (or
    /// itself, when its prefab can't be used). Records never share addresses.
    fn record(
        &mut self,
        doc: &Document,
        id: &RecordId,
        rec: &Record,
        base: &Location,
        out: Option<&mut Vec<Diagnostic>>,
    ) -> Resolved {
        let plain = ResolvedEntity {
            record: rec
                .iter()
                .filter(|(k, _)| is_component_key(k) || k.as_str() == field::NAME)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            parent: parent_of(rec).map(str::to_owned),
            order: order_of(rec).map(str::to_owned),
            source: (doc.id.clone(), id.clone()),
        };
        match prefab_of(rec) {
            None => BTreeMap::from([(id.to_string(), plain)]),
            Some(reference) => {
                let rbase = Location {
                    record: Some(id.to_string()),
                    ..base.clone()
                };
                self.instance(id, rec, reference, &rbase, out)
                    .unwrap_or_else(|| BTreeMap::from([(id.to_string(), plain)]))
            }
        }
    }

    /// Expands one instance record. `None` if the prefab can't be used.
    fn instance(
        &mut self,
        id: &RecordId,
        rec: &Record,
        reference: &str,
        base: &Location,
        mut out: Option<&mut Vec<Diagnostic>>,
    ) -> Option<Resolved> {
        let mut report = |d: Diagnostic| {
            if let Some(out) = out.as_deref_mut() {
                out.push(d.within(base));
            }
        };
        let target = parse_ref(reference).ok().and_then(|p| p.id);
        let Some(prefab) = target.and_then(|t| self.lookup.lookup(t)) else {
            report(
                Diagnostic::error(
                    codes::PREFAB_NOT_FOUND,
                    format!("prefab `{reference}` is not loaded"),
                )
                .path(field::PREFAB),
            );
            return None;
        };
        if self.stack.contains(&prefab.id) {
            report(
                Diagnostic::error(
                    codes::PREFAB_CYCLE,
                    format!("prefab `{reference}` instantiates itself"),
                )
                .path(field::PREFAB),
            );
            return None;
        }
        let Some(root) = prefab.children(None).first().map(|r| r.to_string()) else {
            report(
                Diagnostic::error(
                    codes::ROOT_COUNT,
                    format!("prefab `{reference}` has no root"),
                )
                .path(field::PREFAB),
            );
            return None;
        };
        self.stack.push(prefab.id.clone());
        let inner = self.doc(prefab, &Location::default(), None);
        self.stack.pop();

        let map_addr = |a: &str| {
            if a == root {
                id.to_string()
            } else {
                format!("{id}/{a}")
            }
        };
        let addresses: std::collections::BTreeSet<_> = inner.keys().cloned().collect();
        let mut entities: Resolved = inner
            .into_iter()
            .map(|(addr, mut e)| {
                references::remap(self.db, &mut e.record, &|address| {
                    addresses.contains(address).then(|| map_addr(address))
                });
                if addr == root {
                    e.parent = parent_of(rec).map(str::to_owned);
                    e.order = order_of(rec).map(str::to_owned);
                    if let Some(name) = rec.get(field::NAME) {
                        e.record.insert(field::NAME.into(), name.clone());
                    }
                } else {
                    e.parent = e.parent.as_deref().map(map_addr);
                }
                (map_addr(&addr), e)
            })
            .collect();

        for (i, entry) in rec
            .get(field::REMOVED)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(entry) = entry.as_str() else {
                continue;
            };
            let path = format!("{}[#{i}]", field::REMOVED);
            let (addr, comp) = match entry.rsplit_once('/') {
                Some((ids, c)) if is_component_key(c) => (format!("{id}/{ids}"), Some(c)),
                None if is_component_key(entry) => (id.to_string(), Some(entry)),
                _ => (format!("{id}/{entry}"), None),
            };
            let removed = match comp {
                Some(c) => entities
                    .get_mut(&addr)
                    .and_then(|e| e.record.shift_remove(c))
                    .is_some(),
                None => remove_subtree(&mut entities, &addr),
            };
            if !removed {
                let rest: Vec<Value> = rec[field::REMOVED]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|v| v.as_str() != Some(entry))
                    .cloned()
                    .collect();
                report(
                    Diagnostic::error(
                        codes::REMOVED_TARGET_MISSING,
                        format!("`{entry}` is not in the prefab"),
                    )
                    .path(path)
                    .with_fix(
                        format!("Drop `{entry}`"),
                        FixAction::Set {
                            record: Some(id.to_string()),
                            path: field::REMOVED.into(),
                            value: Value::Array(rest),
                        },
                    ),
                );
            }
        }

        let rt = self.db.get("Entity");
        let mut overrides: Vec<(&String, &Value)> = rec
            .get(field::OVERRIDES)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .collect();
        overrides.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in overrides {
            let opath = format!("{}[{key}]", field::OVERRIDES);
            let remove_fix = || FixAction::Remove {
                record: Some(id.to_string()),
                path: opath.clone(),
            };
            let (ids, fpath) = key
                .rsplit_once('/')
                .map_or(("", key.as_str()), |(i, p)| (i, p));
            let addr = if ids.is_empty() {
                id.to_string()
            } else {
                format!("{id}/{ids}")
            };
            let parsed: Result<FieldPath, String> = fpath.parse();
            let resolved = match (&parsed, rt) {
                (Ok(p), Some(rt)) if is_component_key(p.head()) || p.head() == field::NAME => {
                    self.db.resolve_path(rt, p)
                }
                (Ok(_), _) => Err(Diagnostic::error(
                    codes::INVALID_OVERRIDE_KEY,
                    format!("only `name` and components can be overridden, not `{fpath}`"),
                )),
                (Err(msg), _) => Err(Diagnostic::error(codes::INVALID_OVERRIDE_KEY, msg.clone())),
            };
            let resolved = match resolved {
                Ok(r) => r,
                Err(d) => {
                    report(
                        d.path(opath.clone())
                            .with_fix("Remove the override", remove_fix()),
                    );
                    continue;
                }
            };
            let mut diags = Vec::new();
            Validator::new(self.db, &mut diags).value(&resolved.ty, resolved.field, value, &opath);
            let invalid = diags.iter().any(Diagnostic::is_error);
            for d in diags {
                report(d);
            }
            if invalid {
                continue;
            }
            let Some(entity) = entities.get_mut(&addr) else {
                report(
                    Diagnostic::error(
                        codes::OVERRIDE_TARGET_MISSING,
                        format!("override targets `{ids}`, which is not in the prefab"),
                    )
                    .path(opath.clone())
                    .with_fix("Remove the override", remove_fix()),
                );
                continue;
            };
            if let Err(msg) = path::set(&mut entity.record, &resolved.steps, Some(value.clone())) {
                report(
                    Diagnostic::error(
                        codes::OVERRIDE_TARGET_MISSING,
                        format!("cannot apply override: {msg}"),
                    )
                    .path(opath.clone())
                    .with_fix("Remove the override", remove_fix()),
                );
            }
        }
        Some(entities)
    }
}

/// Removes `addr` and everything parented below it. False if `addr` is missing.
fn remove_subtree(entities: &mut Resolved, addr: &str) -> bool {
    if entities.remove(addr).is_none() {
        return false;
    }
    let kids: Vec<String> = entities
        .iter()
        .filter(|(_, e)| e.parent.as_deref() == Some(addr))
        .map(|(k, _)| k.clone())
        .collect();
    for kid in kids {
        remove_subtree(entities, &kid);
    }
    true
}
