//! The `why` method behind `ara why` and the `ara_why` MCP tool: [`ara_why`]
//! explainers over the host's documents, project files, diagnostics bus and
//! profiler ([`HostFacts`]).
//!
//! Subsystems add explainers and collectors through [`Host::why`](crate::Host::why).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Component;

use ara_diag::store::{Filter, Record};
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, is_component_key};
use ara_profile::SummaryBudget;
use ara_schema::{Schema, SchemaDb};
use ara_why::{Asset, Entity, Facts};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::core::{find_doc, inspect_target, pick};
use crate::host::State;
use crate::service::{Cx, Method, Service, parse};
use crate::text;
use crate::tree::Tree;

/// Params of `why`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhyParams {
    /// Session handle from `session.begin`.
    pub session: String,
    /// What to explain: an entity and symptom (`name:Crate not visible`), a code
    /// (`ARA1302`), an asset (`@meshes/crate.glb`) or a topic (`did reload fall back
    /// to L3`, `is the frame slow`).
    pub question: Option<String>,
    /// Document for entity selectors (default: the one holding the entity, else the
    /// main scene).
    pub doc: Option<String>,
    /// List the explainers and example questions instead of answering.
    pub list: Option<bool>,
}

/// Record keys that are structure, not flags.
const STRUCTURE: &[&str] = &["name", "parent", "order", "prefab", "overrides", "removed"];

/// [`Facts`] over a host's state. Resolved documents are cached for one question.
pub(crate) struct HostFacts<'a> {
    st: &'a State,
    trees: RefCell<BTreeMap<AssetId, std::rc::Rc<Tree>>>,
    profile: Option<Value>,
}

impl<'a> HostFacts<'a> {
    pub(crate) fn new(st: &'a State, profile: Option<Value>) -> Self {
        HostFacts {
            st,
            trees: RefCell::new(BTreeMap::new()),
            profile,
        }
    }

    fn tree(&self, doc: &AssetId) -> Option<std::rc::Rc<Tree>> {
        if let Some(t) = self.trees.borrow().get(doc) {
            return Some(std::rc::Rc::clone(t));
        }
        let tree = std::rc::Rc::new(Tree::build(
            self.st.store.workspace(),
            self.st.store.schema(),
            doc,
        )?);
        self.trees
            .borrow_mut()
            .insert(doc.clone(), std::rc::Rc::clone(&tree));
        Some(tree)
    }

    fn entity_at(&self, tree: &Tree, addr: &str) -> Option<Entity> {
        let e = tree.get(addr)?;
        let db = self.st.store.schema();
        let mut components = BTreeMap::new();
        let mut flags = Map::new();
        for (key, value) in &e.record {
            if is_component_key(key) {
                let fields: Map<String, Value> = text::effective(db, key, value)
                    .into_iter()
                    .map(|(f, v, _)| (f, v))
                    .collect();
                components.insert(key.clone(), Value::Object(fields));
            } else if !STRUCTURE.contains(&key.as_str()) {
                flags.insert(key.clone(), value.clone());
            }
        }
        Some(Entity {
            doc: tree.path.clone(),
            address: addr.to_owned(),
            name: e
                .record
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            parent: tree.parent(addr).map(str::to_owned),
            components,
            flags,
        })
    }
}

/// Finds strings referencing asset `id` anywhere in a value: `(field path, value)`.
fn references(v: &Value, id: &str, path: &str, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.ends_with(&format!("#{id}")) || s == id => out.push(path.to_owned()),
        Value::Object(o) => {
            for (k, v) in o {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                references(v, id, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                references(v, id, &format!("{path}[#{i}]"), out);
            }
        }
        _ => {}
    }
}

impl Facts for HostFacts<'_> {
    fn entity(&self, selector: &str, doc: Option<&str>) -> Result<Entity, Vec<Diagnostic>> {
        // `@levels/a.scene#e_8b21` names its document.
        let (doc, key) = match selector.strip_prefix('@').and_then(|r| r.split_once('#')) {
            Some((path, id)) => (Some(path), id),
            None => (doc, selector),
        };
        let id = match doc {
            Some(d) => find_doc(self.st, d)?,
            None => pick(self.st, None, &[key.to_owned()])?,
        };
        let tree = self.tree(&id).ok_or_else(|| {
            vec![Diagnostic::error(
                codes::DOC_NOT_FOUND,
                format!("`{id}` is not loaded"),
            )]
        })?;
        let addr = inspect_target(&tree, key).map_err(|d| vec![d])?;
        self.entity_at(&tree, &addr).ok_or_else(|| {
            vec![Diagnostic::error(
                codes::RECORD_NOT_FOUND,
                format!("no entity `{key}`"),
            )]
        })
    }

    fn parent(&self, e: &Entity) -> Option<Entity> {
        let id = self.st.store.workspace().find(&e.doc)?.clone();
        let tree = self.tree(&id)?;
        let parent = e.parent.as_deref()?;
        self.entity_at(&tree, parent)
    }

    fn with_component(&self, doc: &str, component: &str) -> Vec<Entity> {
        let Some(id) = self.st.store.workspace().find(doc).cloned() else {
            return Vec::new();
        };
        let Some(tree) = self.tree(&id) else {
            return Vec::new();
        };
        tree.order
            .iter()
            .filter(|(addr, _)| {
                tree.get(addr)
                    .is_some_and(|e| e.record.contains_key(component))
            })
            .filter_map(|(addr, _)| self.entity_at(&tree, addr))
            .collect()
    }

    fn has_component_type(&self, name: &str) -> bool {
        self.st
            .store
            .schema()
            .get(name)
            .is_some_and(|t| t.kind_name() == "component")
    }

    fn asset(&self, reference: &str) -> Asset {
        let text = if reference.starts_with('@') {
            reference.to_owned()
        } else {
            format!("@{reference}")
        };
        let index = self.st.store.workspace().index();
        match ara_doc::refs::resolve(&text, None, index) {
            Ok((canonical, _)) => {
                let (path, id) = canonical
                    .trim_start_matches('@')
                    .split_once('#')
                    .map_or((canonical.clone(), String::new()), |(p, i)| {
                        (p.to_owned(), i.to_owned())
                    });
                let kind = index.kind_of(&id).map(str::to_owned);
                let source_exists = kind.is_none().then(|| self.st.root.join(&path).exists());
                Asset {
                    reference: reference.to_owned(),
                    id: Some(id),
                    path: Some(path),
                    kind,
                    source_exists,
                    problem: None,
                    closest: Vec::new(),
                }
            }
            Err(problem) => Asset {
                reference: reference.to_owned(),
                closest: index.closest(&text, None),
                problem: Some(problem),
                ..Asset::default()
            },
        }
    }

    fn read(&self, path: &str) -> Option<String> {
        let rel = std::path::Path::new(path);
        let inside = rel
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        inside
            .then(|| std::fs::read_to_string(self.st.root.join(rel)).ok())
            .flatten()
    }

    fn users(&self, id: &str, limit: usize) -> Vec<(String, String, String)> {
        let docs: Vec<AssetId> = self
            .st
            .store
            .workspace()
            .docs()
            .map(|(_, d)| d.id.clone())
            .collect();
        let mut out = Vec::new();
        for doc in docs {
            let Some(tree) = self.tree(&doc) else {
                continue;
            };
            for (addr, _) in &tree.order {
                let Some(e) = tree.get(addr) else {
                    continue;
                };
                let mut paths = Vec::new();
                for (key, value) in &e.record {
                    if is_component_key(key) {
                        references(value, id, key, &mut paths);
                    }
                }
                for p in paths {
                    out.push((tree.path.clone(), addr.clone(), p));
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
        out
    }

    fn diagnostics(&self, filter: &Filter) -> Vec<Record> {
        self.st.diag.query(filter).into_iter().cloned().collect()
    }

    fn causes(&self, record: &Record) -> Vec<Record> {
        self.st.diag.causes(record).into_iter().cloned().collect()
    }

    fn profile(&self) -> Option<Value> {
        self.profile.clone()
    }
}

/// The profiler's summary as JSON, if it captured any frames.
fn profile_summary(st: &mut State) -> Option<Value> {
    st.profiler.collect();
    let summary = st
        .profiler
        .profile()
        .summary(SummaryBudget {
            top_n: 20,
            hitches: 5,
            max_bytes: 65_536,
        })
        .ok()?;
    (summary.frames > 0)
        .then(|| serde_json::to_value(summary).ok())
        .flatten()
}

/// The `why` service.
pub(crate) struct WhyService;

impl Service for WhyService {
    fn name(&self) -> &'static str {
        "why"
    }

    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<WhyParams>(
                db,
                "why",
                "Explain a problem: a ranked list of likely causes with evidence and runnable fixes, from the documents and the diagnostics bus.",
            )
            .read_only()
            .tool("ara_why"),
        ]
    }

    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        if method != "why" {
            return Err(vec![Diagnostic::error(
                codes::UNKNOWN_METHOD,
                format!("why does not handle `{method}`"),
            )]);
        }
        let p: WhyParams = parse(params)?;
        let st = &mut *cx.st;
        if p.list.unwrap_or(false) || p.question.as_deref().is_none_or(|q| q.trim().is_empty()) {
            let mut text = String::from("Ask `why` about:\n");
            let list: Vec<Value> = st
                .why
                .list()
                .into_iter()
                .map(|(name, about, examples)| {
                    text.push_str(&format!(
                        "- {name}: {about} e.g. `{}`\n",
                        examples.join("`, `")
                    ));
                    json!({ "name": name, "about": about, "examples": examples })
                })
                .collect();
            return Ok(json!({ "explainers": list, "text": text }));
        }
        let question = p.question.unwrap_or_default();
        let question = match &p.doc {
            Some(doc) if !question.contains("doc:") => format!("{question} doc:{doc}"),
            _ => question,
        };
        let profile = profile_summary(st);
        let st = &*st;
        let facts = HostFacts::new(st, profile);
        let report = st.why.explain(&question, &facts)?;
        let mut out = serde_json::to_value(&report).unwrap_or(Value::Null);
        if let Some(o) = out.as_object_mut() {
            o.insert("text".into(), report.text().into());
        }
        Ok(out)
    }
}
