//! Migration plans are explicit filesystem ops, validated before any source is moved.
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use super::references::{self, Owner};
use ara_diag::{Diagnostic, codes};
use serde::Serialize;
use serde_json::Value;
#[path = "history.rs"]
mod history;

pub(super) const CONTENT_DIRS: &[&str] = &[
    "anim",
    "animation",
    "animations",
    "assets",
    "audio",
    "levels",
    "scenes",
    "look",
    "looks",
    "materials",
    "models",
    "meshes",
    "prefabs",
    "terrain",
    "textures",
    "ui",
    "fonts",
    "world",
    "worlds",
    "vfx",
    "navigation",
    "scripts",
];

/// One reviewable operation in a project layout migration.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum MigrationOp {
    /// Move an existing file or directory, retaining its contents and identity.
    Move {
        /// Absolute source path.
        from: PathBuf,
        /// Absolute destination path.
        to: PathBuf,
    },
    /// Rewrite paths through the canonical JSON writer, or preserve native text markup.
    Rewrite {
        /// Absolute destination path.
        path: PathBuf,
    },
}

/// A validated offline migration. `plan` is read-only; `apply` locks and checks it again.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Migration {
    /// Canonical project root.
    pub root: PathBuf,
    /// Legacy evidence, tests and machine-specific export helpers are retained here.
    pub support: PathBuf,
    #[serde(skip)]
    modern: bool,
    /// Ordered operations, also returned by `ara project migrate --dry-run --json`.
    pub operations: Vec<MigrationOp>,
    #[serde(skip)]
    pub(super) moves: Vec<(PathBuf, PathBuf)>,
    #[serde(skip)]
    pub(super) cleanup: Vec<PathBuf>,
    #[serde(skip)]
    pub(super) rewrites: Vec<Rewrite>,
}

#[derive(Debug)]
pub(super) struct Rewrite {
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
    pub(super) before: Vec<u8>,
    pub(super) after: Vec<u8>,
    pub(super) existed: bool,
}

pub(super) fn problem(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message)
}
pub(super) fn io_error(error: io::Error) -> Diagnostic {
    Diagnostic::error(codes::IO, error.to_string())
}

impl Migration {
    /// Plans a migration without creating directories, locks, or changing any file.
    pub fn plan(root: &Path, support: Option<&Path>) -> Result<Self, Diagnostic> {
        let root = root.canonicalize().map_err(io_error)?;
        if !root.join(crate::project::MARKER).is_file() {
            return Err(crate::project::no_project(&root));
        }
        let marker: Value = ara_json::parse(
            &fs::read_to_string(root.join(crate::project::MARKER)).map_err(io_error)?,
        )
        .map_err(|e| problem(e.message))?;
        if !marker.is_object() {
            return Err(problem("project marker must be a JSON object"));
        }
        let support = support.map(Path::to_path_buf).unwrap_or_else(|| {
            root.with_file_name(format!(
                "{}-support",
                root.file_name().unwrap_or_default().to_string_lossy()
            ))
        });
        let support = std::path::absolute(support).map_err(io_error)?;
        reject_nested_support(&root, &support)?;
        let mut migration = Self {
            root,
            support,
            modern: marker["layoutVersion"]
                .as_u64()
                .is_some_and(|version| version >= super::VERSION),
            operations: Vec::new(),
            moves: Vec::new(),
            cleanup: Vec::new(),
            rewrites: Vec::new(),
        };
        let mut mappings = super::mapping::plan(&mut migration)?;
        migration.move_if_present("ara.imports", "settings/ara.imports")?;
        let presets = migration.root.join("presets");
        if presets.is_dir() {
            let mut profiles = Vec::new();
            collect(&migration.root, &presets, &mut profiles)?;
            for from in profiles {
                let to = migration.root.join("settings/builds").join(
                    from.strip_prefix(&presets)
                        .map_err(|e| problem(e.to_string()))?,
                );
                migration.add_move(from, to)?;
            }
            migration.cleanup.push(presets);
        }
        for entry in fs::read_dir(&migration.root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if entry.file_type().map_err(io_error)?.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".export.json")
            {
                migration.add_move(
                    entry.path(),
                    migration
                        .root
                        .join("settings/builds")
                        .join(entry.file_name()),
                )?;
            }
        }
        for name in ["evidence", "tests", "export.ps1", "windows-build.toml"] {
            let from = migration.root.join(name);
            let to = migration.support.join(name);
            migration.add_move(from, to)?;
        }
        mappings.push(("presets".into(), "settings/builds".into()));
        let mut files = Vec::new();
        collect(&migration.root, &migration.root, &mut files)?;
        let policy = super::preferences(&migration.root)?;
        let resources = policy
            .pointer("/ui/resources")
            .and_then(Value::as_str)
            .map(|path| {
                references::path(
                    path.trim_start_matches('@')
                        .split('#')
                        .next()
                        .unwrap_or(path),
                    &mappings,
                )
            });
        for source in files {
            if source == migration.root.join(super::PREFERENCES)
                || source
                    .file_name()
                    .is_some_and(|name| name == "ara.module.toml")
            {
                continue;
            }
            let target = moved_path(&source, &migration.moves);
            if source
                .extension()
                .is_some_and(|extension| extension == "gltf")
            {
                let before = fs::read(&source).map_err(io_error)?;
                let mut value: Value = serde_json::from_slice(&before)
                    .map_err(|e| problem(format!("{}: {e}", source.display())))?;
                if rewrite_gltf(&mut value, &source, &target, &migration.moves)? {
                    migration.rewrites.push(Rewrite {
                        source,
                        target,
                        before,
                        after: ara_json::to_canonical_string(&value).into_bytes(),
                        existed: true,
                    });
                }
                continue;
            }
            let extension = source
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            // Source media (glTF, SVG, etc.) keeps its relative companions unchanged.
            if !matches!(
                extension,
                "json"
                    | "scene"
                    | "prefab"
                    | "animation"
                    | "anim"
                    | "terrain"
                    | "foliage"
                    | "imports"
                    | "music"
                    | "mixer"
                    | "material"
                    | "materialinstance"
                    | "graph"
                    | "world"
                    | "stories"
                    | "ui"
                    | "css"
                    | "uss"
                    | "md"
                    | "cs"
                    | "toml"
                    | "input"
                    | "nav"
                    | "vfx"
                    | "l10n"
                    | "look"
            ) {
                continue;
            }
            let before = fs::read(&source).map_err(io_error)?;
            let Ok(text) = std::str::from_utf8(&before) else {
                continue;
            };
            let after = if let Ok(mut value) = ara_json::parse(text) {
                let relative = source
                    .strip_prefix(&migration.root)
                    .map_err(|error| problem(error.to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let logical = relative
                    .strip_prefix(&format!("{}/", super::AUTHORING_ROOT))
                    .map(str::to_owned)
                    .unwrap_or_else(|| references::path(&relative, &mappings));
                let owner = if source == migration.root.join(crate::project::MARKER) {
                    Owner::Project
                } else if relative.ends_with(".export.json") {
                    Owner::Export
                } else if logical == "ui/resources.json"
                    || resources.as_deref() == Some(logical.as_str())
                {
                    Owner::Resources
                } else {
                    Owner::Document
                };
                references::document(&mut value, &mappings, owner);
                if target.parent() == Some(migration.root.join("settings/builds").as_path())
                    && target.to_string_lossy().ends_with(".export.json")
                {
                    value["content"] = super::AUTHORING_ROOT.into();
                }
                if source == migration.root.join(crate::project::MARKER) {
                    value["layoutVersion"] = Value::from(super::VERSION);
                    if let Some(fields) = value.as_object_mut() {
                        for key in super::POLICY_KEYS {
                            fields.remove(*key);
                        }
                    }
                }
                if value == ara_json::parse(text).map_err(|e| problem(e.message))? {
                    continue;
                }
                ara_json::to_canonical_string(&value).into_bytes()
            } else if extension == "ui" {
                let relative = source
                    .strip_prefix(&migration.root)
                    .map_err(|error| problem(error.to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let (_,id)=crate::ui_document::header(text,&relative).ok_or_else(||problem(format!("cannot preserve UI identity in {relative}; repair its markup before migrating")))?;
                let pinned = crate::ui_document::with_identity(text, &id)?;
                let rewritten = references::text(&pinned, &mappings, true);
                let canonical = ara_ui_document::SourceDocument::parse(
                    &rewritten,
                    ara_ui_document::DocumentKind::Markup,
                )
                .map_err(|error| problem(error.to_string()))?;
                canonical.serialize().as_bytes().to_vec()
            } else {
                references::text(text, &mappings, false).into_bytes()
            };
            if after != before {
                migration.rewrites.push(Rewrite {
                    source,
                    target,
                    before,
                    after,
                    existed: true,
                });
            }
        }
        migration.update_ignore()?;
        let preferences_path = migration.root.join(super::PREFERENCES);
        let mut preferences = super::preferences(&migration.root)?;
        references::document(&mut preferences, &mappings, Owner::Project);
        materialize_ui(&migration, &mut preferences)?;
        let after = ara_json::to_canonical_string(&ara_json::sort_keys(&preferences)).into_bytes();
        let existed = preferences_path.exists();
        let before = match fs::read(&preferences_path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_error(e)),
        };
        if before != after {
            migration.rewrites.push(Rewrite {
                source: preferences_path.clone(),
                target: preferences_path,
                before,
                after,
                existed,
            });
        }
        let preset = migration.root.join("settings/builds/windows.export.json");
        let preset_source = migration.moves.iter().find_map(|(from, to)| {
            preset.strip_prefix(to).ok().map(|relative| {
                if relative.as_os_str().is_empty() {
                    from.clone()
                } else {
                    from.join(relative)
                }
            })
        });
        if !preset.exists() && !preset_source.is_some_and(|source| source.is_file()) {
            let assembly: String = marker["name"]
                .as_str()
                .unwrap_or("Game")
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            let assembly = if assembly.is_empty() {
                "Game"
            } else {
                &assembly
            };
            let mut value = legacy_build_preset(&migration.root)?
                .unwrap_or_else(|| super::default_build_preset(assembly));
            if let Some(name) = marker.get("name") {
                value["name"] = name.clone();
            }
            value["content"] = super::CONTENT.into();
            migration.rewrites.push(Rewrite {
                source: preset.clone(),
                target: preset,
                before: Vec::new(),
                after: ara_json::to_canonical_string(&value).into_bytes(),
                existed: false,
            });
        }
        migration.rewrite_journal(&mappings)?;
        migration
            .operations
            .extend(migration.rewrites.iter().map(|r| MigrationOp::Rewrite {
                path: r.target.clone(),
            }));
        Ok(migration)
    }

    fn move_if_present(&mut self, from: &str, to: &str) -> Result<(), Diagnostic> {
        self.add_move(self.root.join(from), self.root.join(to))
    }
    pub(super) fn add_move(&mut self, from: PathBuf, to: PathBuf) -> Result<(), Diagnostic> {
        if !from.exists() {
            return Ok(());
        }
        if to.exists()
            || self
                .moves
                .iter()
                .any(|(_, planned)| path_key(planned) == path_key(&to))
        {
            return Err(problem(format!(
                "migration would overwrite `{}`; resolve this collision first",
                to.display()
            )));
        }
        if to
            .ancestors()
            .skip(1)
            .any(|ancestor| ancestor.exists() && !ancestor.is_dir())
        {
            return Err(problem(format!(
                "migration destination has a file where a directory is required: {}",
                to.display()
            )));
        }
        let target = path_key(&to);
        for (source, planned) in &self.moves {
            let planned = path_key(planned);
            if (target.starts_with(&format!("{planned}/")) && source.is_file())
                || (planned.starts_with(&format!("{target}/")) && from.is_file())
            {
                return Err(problem(format!(
                    "migration file/directory collision at {}",
                    to.display()
                )));
            }
        }
        reject_links(&from)?;
        self.operations.push(MigrationOp::Move {
            from: from.clone(),
            to: to.clone(),
        });
        self.moves.push((from, to));
        Ok(())
    }
    fn update_ignore(&mut self) -> Result<(), Diagnostic> {
        let path = self.root.join(".gitignore");
        let before = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_error(e)),
        };
        let mut text =
            String::from_utf8(before.clone()).map_err(|_| problem(".gitignore must be UTF-8"))?;
        for line in [".ara/", "/builds/"] {
            if !text.lines().any(|old| old.trim() == line) {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(line);
                text.push('\n');
            }
        }
        if text.as_bytes() != before {
            let existed = path.exists();
            self.rewrites.push(Rewrite {
                source: path.clone(),
                target: path,
                before,
                after: text.into_bytes(),
                existed,
            });
        }
        Ok(())
    }
    fn rewrite_journal(&mut self, mappings: &[(String, String)]) -> Result<(), Diagnostic> {
        let path = self.root.join(".ara/journal");
        if !path.is_file() {
            return Ok(());
        }
        let before = fs::read(&path).map_err(io_error)?;
        let text = std::str::from_utf8(&before).map_err(|_| problem("journal must be UTF-8"))?;
        let mut snapshots = history::Planner::new(self, mappings, text)?;
        let hashes: BTreeMap<_, _> = self
            .rewrites
            .iter()
            .filter_map(|r| {
                Some((
                    crate::journal::hash(std::str::from_utf8(&r.before).ok()?),
                    crate::journal::hash(std::str::from_utf8(&r.after).ok()?),
                ))
            })
            .collect();
        let mut after = String::new();
        let mut marker_flush = None;
        let marker_before = self
            .rewrites
            .iter()
            .find(|rewrite| rewrite.source == self.root.join(crate::project::MARKER))
            .and_then(|rewrite| std::str::from_utf8(&rewrite.before).ok());
        let legacy_policy = marker_before
            .and_then(|text| ara_json::parse(text).ok())
            .is_some_and(|marker| {
                super::POLICY_KEYS
                    .iter()
                    .any(|key| marker.get(*key).is_some())
            });
        for line in text.lines() {
            let mut value: Value = serde_json::from_str(line)
                .map_err(|e| problem(format!("repair the journal before migrating: {e}")))?;
            snapshots.rewrite_entry(&mut value)?;
            if legacy_policy {
                if value["t"] == "flush" && value["doc"] == "pj_project" {
                    let matches = marker_before
                        .is_some_and(|before| value["hash"] == crate::journal::hash(before));
                    marker_flush = Some((value["rev"].clone(), matches, value["hash"].clone()));
                }
                rewrite_policy_history(&mut value, mappings);
            }
            references::references(&mut value, mappings);
            if let Some(ops) = value
                .get_mut("txn")
                .and_then(|txn| txn.get_mut("ops"))
                .and_then(Value::as_array_mut)
            {
                for op in ops {
                    if matches!(op["op"].as_str(), Some("document" | "file"))
                        && let Some(path) =
                            op.get("path").and_then(Value::as_str).map(str::to_owned)
                        && !path.starts_with("settings/")
                        && path != crate::project::MARKER
                    {
                        let storage = self.history_storage_path(&path, mappings)?;
                        if op["op"] == "file" {
                            op["id"] = ara_doc::file_state::record_id(&storage).to_string().into();
                        }
                        op["path"] = storage.into();
                    }
                }
            }
            if value["t"] == "fileFlush"
                && let Some(path) = value["path"].as_str().map(str::to_owned)
            {
                value["path"] = self.history_storage_path(&path, mappings)?.into();
            }
            if value["t"] == "flush"
                && let Some(new) = value["hash"].as_str().and_then(|hash| hashes.get(hash))
            {
                value["hash"] = new.clone().into();
            }
            // Journal is JSON-lines; canonical key ordering is retained within each entry.
            after.push_str(
                &serde_json::to_string(&ara_json::sort_keys(&value))
                    .map_err(|e| problem(e.to_string()))?,
            );
            after.push('\n');
        }
        let history_writes = snapshots.finish();
        if legacy_policy {
            let preferences = self
                .rewrites
                .iter()
                .find(|rewrite| rewrite.target == self.root.join(super::PREFERENCES))
                .and_then(|rewrite| std::str::from_utf8(&rewrite.after).ok());
            if let Some(preferences) = preferences {
                let (revision, matches, old_hash) =
                    marker_flush.unwrap_or((Value::from(0), true, Value::Null));
                let hash = if matches {
                    crate::journal::hash(preferences).into()
                } else {
                    old_hash
                };
                let flush = serde_json::json!({"t":"flush","doc":super::PREFERENCES_ID,"rev":revision,"hash":hash});
                after.push_str(
                    &serde_json::to_string(&ara_json::sort_keys(&flush))
                        .map_err(|e| problem(e.to_string()))?,
                );
                after.push('\n');
            }
        }
        self.rewrites.extend(history_writes);
        if after.as_bytes() != before {
            self.rewrites.push(Rewrite {
                source: path.clone(),
                target: path,
                before,
                after: after.into_bytes(),
                existed: true,
            });
        }
        Ok(())
    }

    fn history_storage_path(
        &self,
        path: &str,
        mappings: &[(String, String)],
    ) -> Result<String, Diagnostic> {
        let moved = moved_path(&self.root.join(path), &self.moves);
        let original = self.root.join(path);
        if moved != original {
            return moved.strip_prefix(&self.root).map(|path| path.to_string_lossy().replace('\\', "/"))
                .map_err(|_| problem(format!("history refers to support file `{path}` outside the migrated project; archive that history before migrating")));
        }
        if self.modern && path.starts_with(&format!("{}/", super::AUTHORING_ROOT)) {
            return Ok(path.to_owned());
        }
        let logical = references::path(path, mappings);
        if logical.starts_with("settings/")
            || logical.starts_with(".ara/")
            || logical == crate::project::MARKER
        {
            Ok(logical)
        } else {
            Ok(format!("{}/{logical}", super::AUTHORING_ROOT))
        }
    }

    /// Applies the planned operations under the same lock used by the project authority.
    /// A failed write rolls back earlier writes and moves; no original is discarded.
    pub fn apply(&self) -> Result<(), Diagnostic> {
        if self.root.join(".ara").exists() {
            reject_links(&self.root.join(".ara"))?;
        }
        fs::create_dir_all(self.root.join(".ara")).map_err(io_error)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join(".ara/lock"))
            .map_err(io_error)?;
        lock.try_lock().map_err(|e| {
            Diagnostic::error(
                codes::DAEMON_RUNNING,
                format!("close the project before migrating: {e}"),
            )
        })?;
        for rewrite in &self.rewrites {
            let current = fs::read(&rewrite.source).unwrap_or_default();
            if rewrite.source.exists() != rewrite.existed || current != rewrite.before {
                return Err(problem(format!(
                    "`{}` changed since the migration was planned; run again",
                    rewrite.source.display()
                )));
            }
        }
        for (from, to) in &self.moves {
            if !from.exists() || to.exists() {
                return Err(problem(
                    "project changed since the migration was planned; run again",
                ));
            }
            reject_links(from)?;
        }
        let mut moved = Vec::new();
        let mut written = Vec::new();
        let apply = (|| -> io::Result<()> {
            for (from, to) in &self.moves {
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(from, to)?;
                moved.push((from, to));
            }
            for rewrite in &self.rewrites {
                if let Some(parent) = rewrite.target.parent() {
                    fs::create_dir_all(parent)?;
                }
                written.push(rewrite);
                fs::write(&rewrite.target, &rewrite.after)?;
            }
            for root in &self.cleanup {
                remove_empty(root)?;
            }
            super::create_directories(&self.root)?;
            Ok(())
        })();
        if let Err(error) = apply {
            let mut rollback = Vec::new();
            for rewrite in written.into_iter().rev() {
                let result = if rewrite.existed {
                    fs::write(&rewrite.target, &rewrite.before)
                } else {
                    fs::remove_file(&rewrite.target)
                };
                if let Err(e) = result {
                    rollback.push(e.to_string());
                }
            }
            for (from, to) in moved.into_iter().rev() {
                if let Some(parent) = from.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if let Err(e) = fs::rename(to, from) {
                    rollback.push(e.to_string());
                }
            }
            return Err(Diagnostic::error(
                codes::IO,
                format!(
                    "migration failed: {error}; rollback {}",
                    if rollback.is_empty() {
                        "completed".into()
                    } else {
                        rollback.join("; ")
                    }
                ),
            ));
        }
        Ok(())
    }
}

/// Policy history follows its document, so undo cannot put migrated policy back in the marker.
fn rewrite_policy_history(entry: &mut Value, mappings: &[(String, String)]) {
    let Some(ops) = entry
        .get_mut("txn")
        .and_then(|txn| txn.get_mut("ops"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut rewritten = Vec::with_capacity(ops.len());
    for mut operation in std::mem::take(ops) {
        if operation["op"] == "set"
            && matches!(
                operation["doc"].as_str(),
                Some("pj_project" | super::PREFERENCES_ID)
            )
            && let Some(field) = operation["path"].as_str().map(str::to_owned)
        {
            for key in ["value", "prev"] {
                if let Some(value) = operation.get_mut(key) {
                    references::policy_history(value, &field, mappings);
                }
            }
        }
        if operation["op"] == "set"
            && operation["doc"] == "pj_project"
            && operation["id"] == "p_settings"
        {
            let path = operation["path"]
                .as_str()
                .and_then(|path| path.parse::<ara_schema::FieldPath>().ok());
            if let Some(path) = path {
                if path.head() == "settings"
                    && path.segments().get(1).is_some_and(|segment| {
                        let (ara_schema::Segment::Field(key) | ara_schema::Segment::Key(key)) =
                            segment;
                        super::POLICY_KEYS.contains(&key.as_str())
                    })
                {
                    operation["doc"] = super::PREFERENCES_ID.into();
                } else if path.to_string() == "settings" {
                    let mut policy = operation.clone();
                    policy["doc"] = super::PREFERENCES_ID.into();
                    for field in ["value", "prev"] {
                        if let Some(object) =
                            operation.get_mut(field).and_then(Value::as_object_mut)
                        {
                            let mut policy_fields = serde_json::Map::new();
                            for key in super::POLICY_KEYS {
                                if let Some(value) = object.remove(*key) {
                                    policy_fields.insert((*key).to_owned(), value);
                                }
                            }
                            object.insert("layoutVersion".into(), super::VERSION.into());
                            policy[field] = Value::Object(policy_fields);
                        }
                    }
                    rewritten.push(policy);
                }
            }
        }
        rewritten.push(operation);
    }
    *ops = rewritten;
}

fn reject_nested_support(root: &Path, support: &Path) -> Result<(), Diagnostic> {
    let ancestor = support
        .ancestors()
        .find(|p| p.exists())
        .ok_or_else(|| problem("support directory has no existing ancestor"))?;
    let resolved = ancestor.canonicalize().map_err(io_error)?;
    if resolved.starts_with(root)
        || support
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(problem(
            "migration support directory must be outside the game project, with no '..' components",
        ));
    }
    Ok(())
}
fn reject_links(path: &Path) -> Result<(), Diagnostic> {
    let metadata = path.symlink_metadata().map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(problem(format!(
            "migration does not follow symbolic links: {}",
            path.display()
        )));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(io_error)? {
            reject_links(&entry.map_err(io_error)?.path())?;
        }
    }
    Ok(())
}
pub(super) fn collect(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), Diagnostic> {
    let mut entries = fs::read_dir(directory)
        .map_err(io_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error)?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        if (name.to_string_lossy().starts_with('.') && name != ".mcp.json")
            || (directory == root
                && ["target", "node_modules", "builds", "packages"]
                    .iter()
                    .any(|n| name == *n))
        {
            continue;
        }
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_symlink() {
            return Err(problem(format!(
                "migration does not follow symbolic links: {}",
                entry.path().display()
            )));
        }
        if kind.is_dir() {
            if entry.path().join(crate::project::MARKER).is_file() {
                continue;
            }
            collect(root, &entry.path(), files)?;
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}
fn moved_path(path: &Path, moves: &[(PathBuf, PathBuf)]) -> PathBuf {
    moves
        .iter()
        .find_map(|(from, to)| {
            path.strip_prefix(from).ok().map(|rel| {
                if rel.as_os_str().is_empty() {
                    to.clone()
                } else {
                    to.join(rel)
                }
            })
        })
        .unwrap_or_else(|| path.to_owned())
}
/// Read the old sample helper's literal preset without executing any script.
fn legacy_build_preset(root: &Path) -> Result<Option<Value>, Diagnostic> {
    let path = root.join("export.ps1");
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(io_error)?;
    let literal = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("$source = '")
            .and_then(|line| line.strip_suffix('\''))
    });
    let Some(literal) = literal else {
        return Ok(None);
    };
    let literal = literal.replace("[PIN]", "[]");
    let value = ara_json::parse(&literal).map_err(|e| {
        problem(format!(
            "cannot migrate the literal export preset in {}: {}",
            path.display(),
            e.message
        ))
    })?;
    Ok(Some(value))
}

/// Turns the old implicit bootstrap filenames into movable authored policy paths.
fn materialize_ui(migration: &Migration, preferences: &mut Value) -> Result<(), Diagnostic> {
    let source_for = |filename: &str| {
        [
            format!("{}/ui/{filename}", super::AUTHORING_ROOT),
            format!("content/ui/{filename}"),
            format!("assets/ui/{filename}"),
            format!("ui/{filename}"),
        ]
        .into_iter()
        .map(|relative| migration.root.join(relative))
        .find(|path| path.is_file())
    };
    if source_for("game.ui").is_none() && preferences.get("ui").is_none() {
        return Ok(());
    }
    if preferences.get("ui").is_none() {
        preferences["ui"] = serde_json::json!({});
    }
    let ui = preferences
        .get_mut("ui")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| problem("project ui policy must be an object"))?;
    for (key, filename) in [
        ("main", "game.ui"),
        ("stylesheet", "game.css"),
        ("bindings", "bindings.json"),
        ("resources", "resources.json"),
    ] {
        if ui.contains_key(key) {
            continue;
        }
        if let Some(source) = source_for(filename) {
            let target = moved_path(&source, &migration.moves);
            let reference = target
                .strip_prefix(migration.root.join(super::AUTHORING_ROOT))
                .map_err(|error| problem(error.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            ui.insert(key.to_owned(), reference.into());
        }
    }
    Ok(())
}

/// Removes only emptied migration source directories; files left by concurrent writers survive.
fn remove_empty(path: &Path) -> io::Result<()> {
    if !path.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty(&entry.path())?;
        }
    }
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(e) => Err(e),
    }
}

/// Local glTF companions follow their actual moved file, including cross-folder URIs.
fn rewrite_gltf(
    value: &mut Value,
    source: &Path,
    target: &Path,
    moves: &[(PathBuf, PathBuf)],
) -> Result<bool, Diagnostic> {
    let mut changed = false;
    for group in ["buffers", "images"] {
        for item in value
            .get_mut(group)
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let Some(uri) = item.get("uri").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            if uri.contains(':') || uri.starts_with('/') {
                continue;
            }
            let before = lexical(&source.parent().expect("source has parent").join(&uri))?;
            let after = moved_path(&before, moves);
            let relative = relative_uri(target.parent().expect("target has parent"), &after)?;
            if relative != uri {
                item["uri"] = relative.into();
                changed = true;
            }
        }
    }
    Ok(changed)
}
fn lexical(path: &Path) -> Result<PathBuf, Diagnostic> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return Err(problem("source URI escapes its filesystem root"));
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}
fn relative_uri(base: &Path, target: &Path) -> Result<String, Diagnostic> {
    let a = base.components().collect::<Vec<_>>();
    let b = target.components().collect::<Vec<_>>();
    let common = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return Err(problem("source companion moves across filesystem roots"));
    }
    let mut result = PathBuf::new();
    for _ in common..a.len() {
        result.push("..");
    }
    for part in &b[common..] {
        result.push(part.as_os_str());
    }
    Ok(result.to_string_lossy().replace('\\', "/"))
}

pub(super) fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}
