//! A project on disk: the root (marked by `ara.project.json`), the Schema DB
//! (built-in plus `*.types.json`), the command registry, the asset index and the
//! document files.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::project_settings::{read, read_header};
use ara_cmd::Registry;
use ara_diag::{Diagnostic, FixAction, codes};
use ara_doc::{AssetIndex, Document, ReadResult, Workspace};
use ara_schema::SchemaDb;
use serde_json::Value;
mod files;
pub use files::{AuthoredFile, AuthoredRole, AuthoredScope, authored_files, authored_root};

/// The file that marks a project root. Its contents are reserved for project settings.
pub const MARKER: &str = "ara.project.json";
/// Local, uncommitted state: journal, token, daemon file, lock, generated docs.
pub const STATE_DIR: &str = ".ara";
const TYPES_SUFFIX: &str = ".types.json";
const SIDECAR_SUFFIX: &str = ".import.json";
/// Directories never scanned for project files (besides hidden ones).
const SKIPPED: [&str; 3] = ["target", "node_modules", "spikes"];

/// The nearest ancestor of `start` (inclusive) containing `ara.project.json`.
pub fn discover(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join(MARKER).is_file())
        .map(Path::to_path_buf)
}

/// `ARA1901`: no project here, with the `ara init` fix.
pub fn no_project(start: &Path) -> Diagnostic {
    Diagnostic::error(
        codes::NO_PROJECT,
        format!(
            "no `{MARKER}` in `{}` or any parent directory; run `ara init` to create a project, or pass `--root <dir>`",
            start.display()
        ),
    )
    .with_fix(
        "Create a project here with `ara init`",
        FixAction::Cli {
            args: vec!["init".into(), start.display().to_string()],
        },
    )
}

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
    /// `*.types.json` files, as absolute paths.
    pub type_files: Vec<PathBuf>,
    /// Problems with the project's own type definitions and sidecars.
    pub diags: Vec<Diagnostic>,
}

impl Project {
    /// Opens the project at `root`, which must contain `ara.project.json`.
    pub fn open(root: &Path) -> Result<Project, Diagnostic> {
        Self::open_with(root, |_, _| Vec::new())
    }

    /// Like [`Project::open`], with `register` adding engine types, document kinds and
    /// commands first, so project types build on them and files of those kinds load as
    /// documents. Its diagnostics join [`Project::diags`].
    pub fn open_with(
        root: &Path,
        register: impl FnOnce(&mut SchemaDb, &mut Registry) -> Vec<Diagnostic>,
    ) -> Result<Project, Diagnostic> {
        let root = fs::canonicalize(root).map_err(|e| {
            Diagnostic::error(
                codes::IO,
                format!("cannot open project root `{}`: {e}", root.display()),
            )
        })?;
        if !root.join(MARKER).is_file() {
            return Err(no_project(&root));
        }
        let mut db = SchemaDb::new();
        ara_doc::register(&mut db);
        ara_components::register(&mut db);
        let mut registry = Registry::builtin(&mut db);
        let mut engine = crate::ui_document::register(&mut registry, &mut db);
        engine.extend(register(&mut db, &mut registry));

        let mut files = Vec::new();
        walk(&root, &root, &mut files);
        let mut project = Project {
            root,
            db,
            registry,
            index: AssetIndex::new(),
            docs: Vec::new(),
            type_files: Vec::new(),
            diags: engine,
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
                project.type_files.push(file);
            } else if name.ends_with(SIDECAR_SUFFIX) {
                sidecars.push(file);
            } else if project.is_document(&file) {
                project.docs.push(file);
            }
        }
        project.diags.extend(project.db.verify());
        for file in &sidecars {
            project.index_sidecar(file);
        }
        for file in project.docs.clone() {
            if let Ok(text) = fs::read_to_string(&file)
                && let Some((kind, id)) = read_header(&text, &project.rel(&file), &project.db)
            {
                project
                    .index
                    .insert(id.to_string(), project.rel(&file), Some(kind));
            }
        }
        Ok(project)
    }

    /// True if `path` has a registered document extension (`.scene`, `.prefab`).
    pub fn is_document(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| self.db.kinds().any(|k| k.extension == e))
    }

    /// A string field of `ara.project.json`.
    fn marker_field(&self, key: &str) -> Option<String> {
        fs::read_to_string(self.root.join(MARKER))
            .ok()
            .and_then(|t| ara_json::parse(&t).ok())
            .and_then(|v| v.get(key).and_then(Value::as_str).map(str::to_owned))
    }

    /// The project's main scene: `main` in `ara.project.json` (`ara init` writes
    /// `levels/main.scene`), else `levels/main.scene` if it exists. Calls that name no
    /// document default to it.
    pub fn main_scene(&self) -> Option<String> {
        self.marker_field("main").or_else(|| {
            let starter = "levels/main.scene";
            self.root
                .join(starter)
                .is_file()
                .then(|| starter.to_owned())
        })
    }

    /// The project name from `ara.project.json`, else the root directory's name.
    pub fn name(&self) -> String {
        self.marker_field("name").unwrap_or_else(|| {
            self.root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
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
        rel_to(&self.root, path)
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
                        let source = asset_path(&rel, &doc.kind);
                        ws.insert_with_asset_path(rel.clone(), doc, source);
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

    /// Reads a native document format with project reference repair and diagnostics.
    pub fn read_document(&self, text: &str, path: &str) -> ReadResult {
        read(text, path, &self.db, Some(&self.index))
    }

    /// Writes a document in its native canonical format, including lossless UI XML.
    pub fn write_document(&self, doc: &Document) -> Result<String, Diagnostic> {
        crate::project_settings::write(doc, &self.db)
    }
}

/// Reference target for a document; import sidecars describe their source media.
pub(crate) fn asset_path<'a>(path: &'a str, kind: &str) -> &'a str {
    if kind == "import" {
        path.strip_suffix(SIDECAR_SUFFIX).unwrap_or(path)
    } else {
        path
    }
}

/// `path` relative to `root` with `/` separators, or as given if outside it.
pub fn rel_to(root: &Path, path: &Path) -> String {
    let abs = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match abs.strip_prefix(root) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

/// Document files under `root` (registered extensions only), with the same skip
/// rules as opening a project.
pub fn walk_documents(root: &Path, db: &SchemaDb, out: &mut Vec<PathBuf>) {
    out.extend(document_files(root, &document_extensions(db)));
}

/// The file extensions of the registered document kinds.
pub(crate) fn document_extensions(db: &SchemaDb) -> HashSet<String> {
    db.kinds().map(|k| k.extension.clone()).collect()
}

/// Files under `root` with one of `extensions`, in path order (see `walk`).
pub(crate) fn document_files(root: &Path, extensions: &HashSet<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.retain(|f| is_document(f, extensions));
    files
}

/// Whether `path` has one of the document `extensions`.
pub(crate) fn is_document(path: &Path, extensions: &HashSet<String>) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(e))
}

/// Whether `walk` visits the file at `rel` (relative to `root`): no hidden or
/// skipped directory on the way, and no nested project.
pub(crate) fn walked(root: &Path, rel: &Path) -> bool {
    let Some(parent) = rel.parent() else {
        return false;
    };
    let mut dir = root.to_path_buf();
    for part in parent.components() {
        let name = part.as_os_str().to_string_lossy();
        dir.push(part);
        if name.starts_with('.') || SKIPPED.contains(&name.as_ref()) || dir.join(MARKER).is_file() {
            return false;
        }
    }
    true
}

/// Collects files under `dir`. Skips hidden directories (`.git`, `.ara`, `.refs`),
/// build output, `spikes/` and nested projects (directories with their own marker).
fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    // The directory entry knows its type: no metadata call per file (a symlink is
    // followed to tell whether it is a directory).
    let mut entries: Vec<(PathBuf, bool)> = entries
        .filter_map(Result::ok)
        .map(|e| {
            let path = e.path();
            let is_dir = e
                .file_type()
                .is_ok_and(|t| t.is_dir() || (t.is_symlink() && path.is_dir()));
            (path, is_dir)
        })
        .collect();
    entries.sort();
    for (path, is_dir) in entries {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if is_dir {
            let skip = name.starts_with('.')
                || SKIPPED.contains(&name)
                || (path != root && path.join(MARKER).is_file());
            if !skip {
                walk(root, &path, out);
            }
        } else {
            out.push(path);
        }
    }
}
