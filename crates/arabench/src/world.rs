//! The finished project as verifiers see it.
//!
//! [`World::open`] reopens the project in-process after the agent is gone (shutting
//! down a daemon the agent left running), which replays `.ara/journal`, so the state
//! is exact even if the agent was killed before a flush. It exposes resolved entities
//! (prefab instances expanded), the transaction history, `ara check` errors, files and
//! the agent's normalized tool calls.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use ara_diag::{Diagnostic, codes};
use ara_doc::prefab::ResolvedEntity;
use ara_doc::{AssetId, Record};
use ara_host::tree::Tree;
use ara_host::{Host, HostConfig, Project, query};
use ara_ops::Transaction;
use ara_proto::Endpoint;
use ara_schema::SchemaDb;
use serde_json::{Value, json};

use crate::transcript::ToolCall;

/// How long to wait for the agent's engine process to release the project.
const LOCK_WAIT: Duration = Duration::from_secs(20);

/// Label of transactions the host makes from raw file edits.
pub const EXTERNAL_EDIT: &str = "external edit";

/// The finished project.
pub struct World {
    root: PathBuf,
    host: Host,
    trees: BTreeMap<String, Tree>,
    errors: Vec<Diagnostic>,
    /// The agent's tool calls, in order.
    pub calls: Vec<ToolCall>,
}

/// One resolved entity of a document.
#[derive(Clone, Copy)]
pub struct Ent<'a> {
    /// Entity address (`e_8b21`, `e_4f9a/e_m4tz`).
    pub addr: &'a str,
    /// Name and components with overrides applied.
    pub e: &'a ResolvedEntity,
    tree: &'a Tree,
    db: &'a SchemaDb,
}

impl<'a> Ent<'a> {
    /// The entity's name.
    pub fn name(&self) -> &'a str {
        self.e
            .record
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// The effective value at `path` (`Light.intensity`); `None` without the component.
    pub fn get(&self, path: &str) -> Option<Value> {
        query::field(self.db, self.e, path)
    }

    /// The effective number at `path`.
    pub fn num(&self, path: &str) -> Option<f64> {
        self.get(path).and_then(|v| v.as_f64())
    }

    /// The effective number list at `path` (vectors, colors).
    pub fn vec(&self, path: &str) -> Option<Vec<f64>> {
        let v = self.get(path)?;
        v.as_array()?.iter().map(Value::as_f64).collect()
    }

    /// True if the entity has `component` (inline, from its prefab or overridden).
    pub fn has(&self, component: &str) -> bool {
        self.e.record.contains_key(component)
    }

    /// The parent's address.
    pub fn parent(&self) -> Option<&'a str> {
        self.tree.parent(self.addr)
    }

    /// Local position (the origin without a `Transform`).
    pub fn position(&self) -> Vec<f64> {
        self.vec("Transform.position")
            .unwrap_or_else(|| vec![0.0, 0.0, 0.0])
    }
}

impl World {
    /// Opens the finished project at `root`; `calls` are the agent's tool calls.
    pub fn open(root: &Path, calls: Vec<ToolCall>) -> Result<World, String> {
        let host = open_host(root)?;
        let mut world = World {
            root: root.to_path_buf(),
            host,
            trees: BTreeMap::new(),
            errors: Vec::new(),
            calls,
        };
        world.host.flush();
        let store = world.host.store();
        let ws = store.workspace();
        for (path, doc) in ws.docs() {
            if let Some(t) = Tree::build(ws, store.schema(), &doc.id) {
                world.trees.insert(ws.reference_path(path).to_owned(), t);
            }
        }
        world.errors = check_errors(root)?;
        Ok(world)
    }

    /// The project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn tree(&self, doc: &str) -> Option<&Tree> {
        self.trees.get(doc)
    }

    fn ent<'a>(&'a self, tree: &'a Tree, addr: &'a str, e: &'a ResolvedEntity) -> Ent<'a> {
        Ent {
            addr,
            e,
            tree,
            db: self.host.store().schema(),
        }
    }

    /// Every entity of `doc` (an authored-root-relative path), in hierarchy order.
    pub fn entities(&self, doc: &str) -> Vec<Ent<'_>> {
        let Some(tree) = self.tree(doc) else {
            return Vec::new();
        };
        tree.order
            .iter()
            .filter_map(|(addr, _)| tree.get(addr).map(|e| self.ent(tree, addr, e)))
            .collect()
    }

    /// Entities of `doc` named `name`.
    pub fn named(&self, doc: &str, name: &str) -> Vec<Ent<'_>> {
        self.entities(doc)
            .into_iter()
            .filter(|e| e.name() == name)
            .collect()
    }

    /// The entity at `addr` in `doc`.
    pub fn get(&self, doc: &str, addr: &str) -> Option<Ent<'_>> {
        let tree = self.tree(doc)?;
        let (key, e) = tree.entities.get_key_value(addr)?;
        Some(self.ent(tree, key, e))
    }

    /// The children of `addr` (roots for `None`), in sibling order.
    pub fn children(&self, doc: &str, addr: Option<&str>) -> Vec<Ent<'_>> {
        self.entities(doc)
            .into_iter()
            .filter(|e| e.parent() == addr)
            .collect()
    }

    /// The world position of `addr`, if no ancestor rotates or scales (verifiers keep
    /// to such chains).
    pub fn world_pos(&self, doc: &str, addr: &str) -> Option<Vec<f64>> {
        let mut e = self.get(doc, addr)?;
        let mut pos = e.position();
        while let Some(p) = e.parent() {
            e = self.get(doc, p)?;
            let rot = e.vec("Transform.rotation").unwrap_or_default();
            let scale = e.vec("Transform.scale").unwrap_or_else(|| vec![1.0; 3]);
            if rot.iter().any(|r| *r != 0.0) || scale.iter().any(|s| *s != 1.0) {
                return None;
            }
            for (a, b) in pos.iter_mut().zip(e.position()) {
                *a += b;
            }
        }
        Some(pos)
    }

    /// The stored document record (not resolved): `prefab`, `overrides`, `removed`.
    pub fn record(&self, doc: &str, id: &str) -> Option<&Record> {
        let ws = self.host.store().workspace();
        ws.doc(ws.find(doc)?.as_str())?.record(id)
    }

    /// The asset ID of the document at `path`.
    pub fn doc_id(&self, path: &str) -> Option<AssetId> {
        self.host.store().workspace().find(path).cloned()
    }

    /// Every transaction, oldest first.
    pub fn history(&self) -> &[Transaction] {
        self.host.store().history()
    }

    /// Transactions whose ops touch record `id` of document `doc`.
    pub fn txns_touching(&self, doc: &str, id: &str) -> Vec<&Transaction> {
        let Some(doc_id) = self.doc_id(doc) else {
            return Vec::new();
        };
        self.history()
            .iter()
            .filter(|t| {
                t.ops
                    .iter()
                    .any(|op| *op.doc() == doc_id && op.id().as_str() == id)
            })
            .collect()
    }

    /// True if a later transaction undid `txn` (and was not itself undone).
    pub fn reverted(&self, txn: &str) -> bool {
        self.history()
            .iter()
            .find(|t| t.id.0.as_str() == txn)
            .is_some_and(|t| self.host.store().reverted_by(&t.id).is_some())
    }

    /// Transactions the host made from raw file edits.
    pub fn external_edits(&self) -> usize {
        self.history()
            .iter()
            .filter(|t| t.label.as_deref() == Some(EXTERNAL_EDIT))
            .count()
    }

    /// Branch sessions persisted in `.ara/branches/`, rebuilt over the live
    /// documents (their own state is `branch.store()`).
    pub fn branches(&self) -> Vec<ara_collab::Branch> {
        let dir = self.root.join(".ara/branches");
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| serde_json::from_str::<ara_collab::SavedBranch>(&t).ok())
            .filter_map(|b| ara_collab::Branch::load(b, self.host.store()).ok())
            .collect()
    }

    /// `ara check` errors over every project document.
    pub fn errors(&self) -> &[Diagnostic] {
        &self.errors
    }

    /// An authored-root-relative file's text.
    pub fn authored_file(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(crate::fixture::authored_path(&self.root, rel)).ok()
    }

    /// A project file's text.
    pub fn file(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel)).ok()
    }

    /// `git <args>` in the project; its stdout, or `None` if it failed.
    pub fn git(&self, args: &[&str]) -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Opens the project's host, waiting for the agent's engine to exit and stopping a
/// daemon the agent left running.
fn open_host(root: &Path) -> Result<Host, String> {
    let start = Instant::now();
    let mut asked = false;
    loop {
        let project = Project::open(root).map_err(|d| d.to_string())?;
        match Host::open(project, HostConfig::default()) {
            Ok(h) => return Ok(h),
            Err(d) if d.code == codes::DAEMON_RUNNING && start.elapsed() < LOCK_WAIT => {
                if !asked && let Some(mut c) = ara_rpc::Client::for_project(root) {
                    asked = true;
                    let _ = shutdown(&mut c);
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(d) => return Err(d.to_string()),
        }
    }
}

fn shutdown(ep: &mut dyn Endpoint) -> Result<(), String> {
    let begun = ep
        .call(
            "session.begin",
            json!({ "client": "arabench", "author": "human" }),
        )
        .map_err(|e| e.to_string())?;
    let session = begun["session"].as_str().unwrap_or_default().to_owned();
    ep.call("daemon.shutdown", json!({ "session": session }))
        .map(drop)
        .map_err(|e| e.to_string())
}

/// What `ara check` reports as errors: reader, structure, reference and prefab
/// problems of every document, plus project type problems.
fn check_errors(root: &Path) -> Result<Vec<Diagnostic>, String> {
    let project = Project::open(root).map_err(|d| d.to_string())?;
    let (ws, per_file) = project.load(&project.docs);
    let mut diags = project.diags.clone();
    diags.extend(per_file.into_values().flatten());
    for file in &project.docs {
        if let Some(id) = ws.find(&project.rel(file)).cloned() {
            ws.resolve(&id, &project.db, &mut diags);
        }
    }
    diags.retain(Diagnostic::is_error);
    Ok(diags)
}
