//! A document's entities after prefab resolution, in hierarchy order. Queries,
//! `inspect` and the scene-to-text summary all walk this.

use std::collections::{BTreeMap, BTreeSet};

use ara_doc::prefab::{Resolved, ResolvedEntity};
use ara_doc::{AssetId, Workspace, prefab_of};
use ara_schema::SchemaDb;

/// One document's resolved entities.
pub struct Tree {
    /// Asset ID.
    pub doc: AssetId,
    /// Project-relative path.
    pub path: String,
    /// Document kind (`scene`).
    pub kind: String,
    /// Entities by address.
    pub entities: Resolved,
    /// Addresses in depth-first hierarchy order, with their depth (roots are 0).
    pub order: Vec<(String, u32)>,
    parents: BTreeMap<String, String>,
}

impl Tree {
    /// Resolves a loaded document. Resolution problems are left to `ara check`.
    pub fn build(ws: &Workspace, db: &SchemaDb, doc: &AssetId) -> Option<Tree> {
        let d = ws.doc(doc.as_str())?;
        let mut ignored = Vec::new();
        let entities = ws.resolve(doc, db, &mut ignored);
        let mut kids: BTreeMap<Option<&str>, Vec<(&str, &str)>> = BTreeMap::new();
        let mut parents = BTreeMap::new();
        for (addr, e) in &entities {
            let parent = e
                .parent
                .as_deref()
                .filter(|p| entities.contains_key(*p) && *p != addr);
            if let Some(p) = parent {
                parents.insert(addr.clone(), p.to_owned());
            }
            kids.entry(parent)
                .or_default()
                .push((e.order.as_deref().unwrap_or(""), addr.as_str()));
        }
        for list in kids.values_mut() {
            list.sort();
        }
        let mut order = Vec::new();
        let mut seen = BTreeSet::new();
        let mut stack: Vec<(&str, u32)> = kids
            .get(&None)
            .map(|l| l.iter().rev().map(|(_, a)| (*a, 0)).collect())
            .unwrap_or_default();
        while let Some((addr, depth)) = stack.pop() {
            if !seen.insert(addr) {
                continue;
            }
            order.push((addr.to_owned(), depth));
            if let Some(list) = kids.get(&Some(addr)) {
                stack.extend(list.iter().rev().map(|(_, a)| (*a, depth + 1)));
            }
        }
        // Entities caught in a parent loop are unreachable from the roots; list them last.
        for addr in entities.keys() {
            if !seen.contains(addr.as_str()) {
                order.push((addr.clone(), 0));
            }
        }
        Some(Tree {
            doc: doc.clone(),
            path: ws.path_of(doc.as_str()).unwrap_or_default().to_owned(),
            kind: d.kind.clone(),
            entities,
            order,
            parents,
        })
    }

    /// One entity.
    pub fn get(&self, addr: &str) -> Option<&ResolvedEntity> {
        self.entities.get(addr)
    }

    /// The parent address.
    pub fn parent(&self, addr: &str) -> Option<&str> {
        self.parents.get(addr).map(String::as_str)
    }

    /// True if `addr` is strictly below `ancestor`.
    pub fn is_under(&self, addr: &str, ancestor: &str) -> bool {
        let mut cur = self.parent(addr);
        let mut steps = 0;
        while let Some(p) = cur {
            if p == ancestor {
                return true;
            }
            steps += 1;
            if steps > self.entities.len() {
                return false;
            }
            cur = self.parent(p);
        }
        false
    }

    /// The short prefab name of an instance root in this document (`crate` for
    /// `@prefabs/crate.prefab#pf_...`).
    pub fn prefab_name(&self, ws: &Workspace, addr: &str) -> Option<String> {
        if addr.contains('/') {
            return None;
        }
        let r = ws.doc(self.doc.as_str())?.record(addr)?;
        let reference = prefab_of(r)?;
        let path = reference
            .trim_start_matches('@')
            .split('#')
            .next()
            .unwrap_or_default();
        let file = path.rsplit('/').next().unwrap_or(path);
        Some(file.split('.').next().unwrap_or(file).to_owned())
    }

    /// The document's display name: its file stem.
    pub fn name(&self) -> &str {
        let file = self.path.rsplit('/').next().unwrap_or(&self.path);
        file.split('.').next().unwrap_or(file)
    }
}
