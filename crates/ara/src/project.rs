//! Opening a project from disk: the root, the Schema DB (built-in plus `*.types.json`),
//! the command registry, the asset index and the documents.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ara_cmd::Registry;
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetIndex, Workspace, read, read_header};
use ara_schema::SchemaDb;
use serde_json::Value;

/// The file that marks a project root. Its contents are reserved for project settings.
pub const MARKER: &str = "ara.project.json";
const TYPES_SUFFIX: &str = ".types.json";
const SIDECAR_SUFFIX: &str = ".import.json";

/// A project on disk.
pub struct Project {
    /// Project root; references are relative to it.
    pub root: PathBuf,
    /// Built-in and project types.
    pub db: SchemaDb,
    /// Built-in commands.
    pub registry: Registry,
    /// Every document and imported asset, by ID and path.
    pub index: AssetIndex,
    /// Project documents, as absolute paths.
    pub docs: Vec<PathBuf>,
    /// Problems with the project's own type definitions.
    pub diags: Vec<Diagnostic>,
}

impl Project {
    /// Opens the project at `root`, or at the nearest ancestor of the current
    /// directory containing `ara.project.json` (falling back to the current directory).
    pub fn open(root: Option<&Path>) -> Result<Project, String> {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("cannot read the current directory: {e}"))?;
        let root = match root {
            Some(r) => r.to_path_buf(),
            None => cwd
                .ancestors()
                .find(|d| d.join(MARKER).is_file())
                .unwrap_or(&cwd)
                .to_path_buf(),
        };
        let root = fs::canonicalize(&root)
            .map_err(|e| format!("cannot open project root `{}`: {e}", root.display()))?;

        let mut db = SchemaDb::new();
        ara_doc::register(&mut db);
        ara_components::register(&mut db);
        let registry = Registry::builtin(&mut db);
        let extensions: Vec<String> = db.kinds().map(|k| k.extension.clone()).collect();

        let mut files = Vec::new();
        walk(&root, &mut files);
        let mut project = Project {
            root,
            db,
            registry,
            index: AssetIndex::new(),
            docs: Vec::new(),
            diags: Vec::new(),
        };
        let mut sidecars = Vec::new();
        for file in files {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            if name.ends_with(TYPES_SUFFIX) {
                project.load_types(&file);
            } else if name.ends_with(SIDECAR_SUFFIX) {
                sidecars.push(file);
            } else if file
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.iter().any(|x| x == e))
            {
                project.docs.push(file);
            }
        }
        project.diags.extend(project.db.verify());
        for file in &sidecars {
            project.index_sidecar(file);
        }
        for file in project.docs.clone() {
            if let Ok(text) = fs::read_to_string(&file)
                && let Some((kind, id)) = read_header(&text, &project.db)
            {
                project
                    .index
                    .insert(id.to_string(), project.rel(&file), Some(kind));
            }
        }
        Ok(project)
    }

    fn load_types(&mut self, file: &Path) {
        let rel = self.rel(file);
        let parsed = fs::read_to_string(file)
            .map_err(|e| Diagnostic::error(codes::IO, format!("cannot read: {e}")))
            .and_then(|t| {
                ara_json::parse(&t).map_err(|e| {
                    Diagnostic::error(codes::JSON_SYNTAX, e.message).line_col(e.line, e.column)
                })
            });
        match parsed {
            Ok(v) => self.diags.extend(
                self.db
                    .add_json_types(&v)
                    .into_iter()
                    .map(|d| d.file(rel.clone())),
            ),
            Err(d) => self.diags.push(d.file(rel)),
        }
    }

    fn index_sidecar(&mut self, file: &Path) {
        let rel = self.rel(file);
        let id = fs::read_to_string(file)
            .ok()
            .and_then(|t| ara_json::parse(&t).ok())
            .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_owned));
        match id {
            Some(id) if ara_doc::AssetId::parse(&id).is_ok() => {
                let source = rel.strip_suffix(SIDECAR_SUFFIX).unwrap_or(&rel).to_owned();
                self.index.insert(id, source, None);
            }
            _ => self.diags.push(
                Diagnostic::error(codes::MISSING_DOC_ID, "import sidecar needs an asset `id`")
                    .file(rel),
            ),
        }
    }

    /// `path` relative to the root with `/` separators, or as given if outside it.
    pub fn rel(&self, path: &Path) -> String {
        let abs = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        match abs.strip_prefix(&self.root) {
            Ok(rel) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
            Err(_) => path.display().to_string(),
        }
    }

    /// Reads `extra` files plus every project document into a workspace. Returns the
    /// reader diagnostics per file (by relative path).
    pub fn load(&self, extra: &[PathBuf]) -> (Workspace, BTreeMap<String, Vec<Diagnostic>>) {
        let mut ws = Workspace::new(self.index.clone());
        let mut diags = BTreeMap::new();
        let mut seen = Vec::new();
        for file in self.docs.iter().chain(extra) {
            let rel = self.rel(file);
            if seen.contains(&rel) {
                continue;
            }
            seen.push(rel.clone());
            match fs::read_to_string(file) {
                Ok(text) => {
                    let result = read(&text, &rel, &self.db, Some(&self.index));
                    if let Some(doc) = result.doc {
                        ws.insert(rel.clone(), doc);
                    }
                    diags.insert(rel, result.diags);
                }
                Err(e) => {
                    diags.insert(
                        rel.clone(),
                        vec![Diagnostic::error(codes::IO, format!("cannot read: {e}")).file(rel)],
                    );
                }
            }
        }
        (ws, diags)
    }
}

/// Collects files under `dir`, skipping hidden directories and build output.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                walk(&path, out);
            }
        } else {
            out.push(path);
        }
    }
}
