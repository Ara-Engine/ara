//! What explainers can read. The project host implements [`Facts`] over its
//! documents, project files, diagnostics bus and profiler; tests implement it over
//! fixtures.

use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use ara_diag::store::{Filter, Record};
use serde_json::{Map, Value};

/// One entity after prefab resolution.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entity {
    /// Project-relative path of its document (`levels/outpost.scene`).
    pub doc: String,
    /// Address in the document (`e_8b21`, `e_4f9a/e_m4tz` inside an instance).
    pub address: String,
    /// Its `name`, if any.
    pub name: Option<String>,
    /// Address of its parent.
    pub parent: Option<String>,
    /// Components with every schema field present (defaults filled in).
    pub components: BTreeMap<String, Value>,
    /// Record keys that are not components, `name`, `parent`, `order` or `prefab`
    /// (flags such as `enabled`).
    pub flags: Map<String, Value>,
}

impl Entity {
    /// `Crate (e_crate001)`, or just the address.
    pub fn label(&self) -> String {
        match &self.name {
            Some(n) => format!("`{n}` ({})", self.address),
            None => format!("`{}`", self.address),
        }
    }

    /// A field of a component (`get("Transform", "scale")`).
    pub fn get(&self, component: &str, field: &str) -> Option<&Value> {
        self.components.get(component)?.get(field)
    }

    /// The first component whose name is one of `names` (case-insensitive), or ends
    /// with `suffix` when given.
    pub fn component(&self, names: &[&str], suffix: Option<&str>) -> Option<(&str, &Value)> {
        self.components
            .iter()
            .find(|(k, _)| names.iter().any(|n| n.eq_ignore_ascii_case(k)))
            .or_else(|| {
                let suffix = suffix?;
                self.components.iter().find(|(k, _)| k.ends_with(suffix))
            })
            .map(|(k, v)| (k.as_str(), v))
    }
}

/// What a reference resolves to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Asset {
    /// The reference as written (`@meshes/crate.glb#ms_crate001`).
    pub reference: String,
    /// Asset ID, when it resolves.
    pub id: Option<String>,
    /// Project-relative path, when it resolves.
    pub path: Option<String>,
    /// Document kind (`scene`, `material`), `None` for imported source media.
    pub kind: Option<String>,
    /// For imported media: whether the source file exists next to its sidecar.
    pub source_exists: Option<bool>,
    /// Why it does not resolve (`ARA1302`, `ARA1305`).
    pub problem: Option<Diagnostic>,
    /// When it does not resolve: the closest known assets of the same kind, as
    /// `@path#id` (several when tied).
    pub closest: Vec<String>,
}

impl Asset {
    /// True when the reference resolves.
    pub fn resolves(&self) -> bool {
        self.problem.is_none() && self.id.is_some()
    }

    /// `@path#id` when resolved, else the reference as written.
    pub fn canonical(&self) -> String {
        match (&self.path, &self.id) {
            (Some(p), Some(id)) => format!("@{p}#{id}"),
            _ => self.reference.clone(),
        }
    }
}

/// Read access to the project for explainers.
pub trait Facts {
    /// Resolves an entity selector (`name:Crate`, `e_8b21`, `e_4f9a/e_m4tz`,
    /// `@levels/a.scene#e_8b21`) in `doc`, else in the document holding it, else in
    /// the main scene. Absence and ambiguity are diagnostics with fixes.
    fn entity(&self, selector: &str, doc: Option<&str>) -> Result<Entity, Vec<Diagnostic>>;
    /// The entity's parent.
    fn parent(&self, entity: &Entity) -> Option<Entity>;
    /// Every entity of document `doc` that has `component`.
    fn with_component(&self, doc: &str, component: &str) -> Vec<Entity>;
    /// True if the project's schema has a component type of this name.
    fn has_component_type(&self, name: &str) -> bool;
    /// Resolves an asset reference (`@path#id`, `@path`, or a bare path or ID).
    fn asset(&self, reference: &str) -> Asset;
    /// A project file's text (project-relative path).
    fn read(&self, path: &str) -> Option<String>;
    /// Entities whose fields reference asset `id`: `(doc, entity, field path)`.
    fn users(&self, id: &str, limit: usize) -> Vec<(String, String, String)>;
    /// Bus records matching `filter`, oldest first.
    fn diagnostics(&self, filter: &Filter) -> Vec<Record>;
    /// What caused a record, nearest first.
    fn causes(&self, record: &Record) -> Vec<Record>;
    /// The latest profile summary (`ara_profile::Summary` as JSON), if one exists.
    fn profile(&self) -> Option<Value>;
}

/// The chain of ancestors of `e`, nearest first (stops at 64 or a loop).
pub fn ancestors(facts: &dyn Facts, e: &Entity) -> Vec<Entity> {
    let mut out: Vec<Entity> = Vec::new();
    let mut cur = facts.parent(e);
    while let Some(p) = cur {
        if out.len() >= 64 || p.address == e.address || out.iter().any(|a| a.address == p.address) {
            break;
        }
        cur = facts.parent(&p);
        out.push(p);
    }
    out
}
