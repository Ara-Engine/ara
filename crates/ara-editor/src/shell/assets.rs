use ara_host::project::{AuthoredFile, AuthoredRole, AuthoredScope, authored_files};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
};
#[cfg(test)]
pub(in crate::shell) mod scan_fixture;
#[cfg(test)]
mod scan_tests;
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Asset {
    /// Project-relative storage path: browser identity, selection and physical IO.
    pub path: String,
    /// Logical authored path for typed asset commands and references; absent for project files.
    pub authored_path: Option<String>,
    /// Registered document kind, independent of the file's extension.
    pub document_kind: Option<String>,
    pub reference: Option<String>,
    pub kind: String,
    pub domain: Option<String>,
    pub uses: Vec<String>,
    pub stamp: u64,
}
/// What an asset field of declared `kind` at `field` takes, as `(extensions, phrase)`:
/// a document kind takes its own files, `audio` the audio formats, `MeshRenderer.model`
/// model sources and `material` fields material graphs; other fields take any asset
/// (empty list).
pub(crate) fn field_accepts(
    kind: Option<&str>,
    field: &str,
) -> (&'static [&'static str], &'static str) {
    match kind {
        Some("audio") => (&["wav", "ogg", "mp3", "flac"], "an audio file"),
        Some("prefab") => (&["prefab"], "a prefab"),
        Some("scene") => (&["scene"], "a scene"),
        Some("animation") => (&["animation"], "an animation"),
        Some("graph") => (&["graph"], "a graph"),
        Some("terrain") => (&["terrain"], "a terrain"),
        Some("foliage") => (&["foliage"], "a foliage set"),
        Some("music") => (&["music"], "a music document"),
        Some("world") => (&["world"], "a world"),
        Some(_) => (&[], "an asset"),
        None if field.ends_with(".model") => (&["gltf", "glb", "fbx"], "a glTF or FBX model"),
        None if field.rsplit(['.', '/']).next() == Some("material") => {
            (&["material", "graph"], "a material")
        }
        None => (&[], "an asset"),
    }
}
impl Asset {
    pub(super) fn unindexed(path: String, kind: String) -> Self {
        Self {
            path,
            authored_path: None,
            document_kind: None,
            reference: None,
            kind,
            domain: None,
            uses: Vec::new(),
            stamp: 0,
        }
    }
    pub(super) fn folder(path: String, authored_path: Option<String>) -> Self {
        Self {
            authored_path,
            ..Self::unindexed(path, "folder".into())
        }
    }
    /// Whether this file can fill an asset field of declared `kind` at `field`
    /// ([`field_accepts`]); folders never do.
    pub fn fits(&self, kind: Option<&str>, field: &str) -> bool {
        let (extensions, _) = field_accepts(kind, field);
        self.kind != "folder" && (extensions.is_empty() || extensions.contains(&self.kind.as_str()))
    }
    /// The file name, for messages.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
    /// Prefer the authority's current index when a recent import predates the browser scan.
    pub fn current_reference(&self, workspace: &ara_doc::Workspace) -> Option<String> {
        let path = self.authored_path.as_deref()?;
        workspace
            .index()
            .id_of(path)
            .map(|id| format!("@{path}#{id}"))
            .or_else(|| self.reference.clone())
    }
}
struct Snapshot {
    files: Vec<AuthoredFile>,
    items: Vec<Asset>,
}
/// A scan already started when an authority save completed cannot acknowledge it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScanBarrier(u64);
pub(super) struct Browser {
    pub items: Vec<Asset>,
    /// Last successful semantic index, also used to resolve OS-drop destinations.
    pub files: Vec<AuthoredFile>,
    /// Workspace reference prefix, empty for legacy projects.
    reference_root: String,
    /// Bumped whenever rows or their diagnostic change.
    pub revision: u64,
    pub error: Option<String>,
    pub(super) diagnostic: Option<ara_diag::Diagnostic>,
    pending: Receiver<(u64, Result<Snapshot, ara_diag::Diagnostic>)>,
    scan_started: Arc<AtomicU64>,
    scan_completed: u64,
    refresh: mpsc::SyncSender<()>,
    optimistic: std::collections::BTreeMap<String, Option<Asset>>,
}
impl Browser {
    /// Physical reference prefix configured by the loaded workspace.
    pub(super) fn reference_root(&self) -> &str {
        &self.reference_root
    }
    /// Legacy source creation publishes only its already-completed command output.
    /// File reorganization never uses these optimistic rows.
    pub(super) fn insert_path(&mut self, path: &str, kind: &str) {
        self.remove_path(path);
        let mut asset = Asset::unindexed(path.into(), kind.into());
        // This legacy seam is used only after a typed source-creation command
        // succeeded (and by its focused fixtures), never for file operations.
        asset.authored_path = Some(
            self.files
                .iter()
                .find(|entry| {
                    entry.storage_path == path && matches!(&entry.scope, AuthoredScope::Authored)
                })
                .map(|entry| entry.path.clone())
                .unwrap_or_else(|| {
                    path.strip_prefix(&self.reference_root)
                        .and_then(|tail| tail.strip_prefix('/'))
                        .unwrap_or(path)
                        .to_owned()
                }),
        );
        self.items.push(asset);
        if let Some(asset) = self.items.last() {
            self.optimistic.insert(path.into(), Some(asset.clone()));
        }
        self.items.sort_by(|a, b| a.path.cmp(&b.path));
        self.revision += 1;
        self.refresh();
    }
    pub(super) fn remove_path(&mut self, path: &str) {
        self.items.retain(|asset| asset.path != path);
        self.optimistic.insert(path.into(), None);
        self.revision += 1;
    }
    pub fn new(root: &Path, schema: &ara_schema::SchemaDb, reference_root: &str) -> Self {
        let root = root.to_owned();
        let schema = schema.clone();
        let (tx, pending) = mpsc::sync_channel(1);
        let (refresh, wake) = mpsc::sync_channel(1);
        let scan_started = Arc::new(AtomicU64::new(0));
        let worker_started = Arc::clone(&scan_started);
        std::thread::spawn(move || {
            let mut cache = std::collections::HashMap::new();
            let mut generation = 0_u64;
            loop {
                generation = generation.saturating_add(1);
                worker_started.store(generation, Ordering::Release);
                let snapshot = scan(&root, &schema, &mut cache);
                if tx.send((generation, snapshot)).is_err() {
                    break;
                }
                match wake.recv_timeout(std::time::Duration::from_secs(2)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Self {
            items: Vec::new(),
            files: Vec::new(),
            reference_root: reference_root.into(),
            revision: 0,
            error: None,
            diagnostic: None,
            pending,
            scan_started,
            scan_completed: 0,
            refresh,
            optimistic: Default::default(),
        }
    }
    /// Request a fresh semantic projection after a completed authority job.
    pub(super) fn refresh(&self) {
        let _ = self.refresh.try_send(());
    }
    /// Request a scan which starts after this call. Call only after the authority
    /// has saved; a queued or currently running older scan must not satisfy it.
    pub(super) fn refresh_after_current_scan(&self) -> ScanBarrier {
        let barrier = ScanBarrier(self.scan_started.load(Ordering::Acquire));
        self.refresh();
        barrier
    }
    /// An equal-content successful scan still acknowledges a refresh barrier.
    pub(super) fn completed_scan_after(&self, barrier: ScanBarrier) -> bool {
        self.error.is_none() && self.scan_completed > barrier.0
    }
    pub fn poll(&mut self) -> bool {
        let (generation, result) = match self.pending.try_recv() {
            Ok(result) => result,
            Err(_) => return false,
        };
        self.scan_completed = generation;
        let mut snapshot = match result {
            Ok(snapshot) => snapshot,
            Err(problem) => {
                let message = problem.message.clone();
                let changed = self.error.as_deref() != Some(message.as_str());
                self.error = Some(message);
                if changed {
                    self.diagnostic = Some(problem);
                }
                self.revision += u64::from(changed);
                return changed;
            }
        };
        self.optimistic.retain(|path, asset| {
            let found = snapshot.items.iter().position(|asset| asset.path == *path);
            match (asset.as_ref(), found) {
                (Some(_), Some(_)) | (None, None) => false,
                (Some(asset), None) => {
                    snapshot.items.push(asset.clone());
                    true
                }
                (None, Some(index)) => {
                    snapshot.items.remove(index);
                    true
                }
            }
        });
        snapshot.items.sort_by(|a, b| a.path.cmp(&b.path));
        let recovered = self.error.take().is_some();
        let changed = self.items != snapshot.items || recovered;
        self.items = snapshot.items;
        self.files = snapshot.files;
        self.revision += u64::from(changed);
        changed
    }
    /// A cached storage folder resolves to an authored command path only through
    /// the authority index; project settings folders cannot become content targets.
    pub(super) fn authored_folder(&self, storage: &str) -> Result<String, String> {
        self.authored_folder_ref(storage).map(str::to_owned)
    }
    pub(super) fn authored_folder_ref(&self, storage: &str) -> Result<&str, String> {
        cached_authored_folder(&self.files, &self.reference_root, storage)
    }
}
fn cached_authored_folder<'a>(
    files: &'a [AuthoredFile],
    reference_root: &str,
    storage: &str,
) -> Result<&'a str, String> {
    if storage.is_empty() {
        return Ok("");
    }
    if let Some(entry) = files.iter().find(|entry| entry.storage_path == storage) {
        if !matches!(&entry.scope, AuthoredScope::Authored)
            || !matches!(&entry.role, AuthoredRole::Folder)
        {
            return Err("Choose a folder inside authored content; project settings are not asset destinations".into());
        }
        return Ok(entry.path.as_str());
    }
    if !reference_root.is_empty() && storage == reference_root {
        return Ok("");
    }
    Err("This folder is not in the current authored index".into())
}
impl super::Shell {
    /// Retry the cached semantic projection without starting an authoring job.
    pub(crate) fn refresh_asset_browser(&mut self) -> ara_ui::Result<()> {
        self.browser.refresh();
        self.asset_grid_dirty = true;
        Ok(())
    }
    /// The last successful semantic index is immutable during a UI/drop dispatch.
    pub(crate) fn asset_authored_files(&self) -> &[AuthoredFile] {
        &self.browser.files
    }
}
fn scan(
    root: &Path,
    schema: &ara_schema::SchemaDb,
    cache: &mut std::collections::HashMap<String, Asset>,
) -> Result<Snapshot, ara_diag::Diagnostic> {
    let files = authored_files(root, schema)?;
    let mut items = Vec::with_capacity(files.len());
    for entry in &files {
        if entry.storage_path.ends_with(".import.json") {
            continue;
        }
        let authored_path =
            matches!(&entry.scope, AuthoredScope::Authored).then(|| entry.path.clone());
        if matches!(&entry.role, AuthoredRole::Folder) {
            items.push(Asset::folder(entry.storage_path.clone(), authored_path));
            continue;
        }
        let document_kind = match &entry.role {
            AuthoredRole::Document { name } => Some(name.clone()),
            _ => None,
        };
        let path = root.join(&entry.storage_path);
        let stamp = stamp(&path);
        if let Some(known) = cache.get(&entry.storage_path).filter(|asset| {
            asset.stamp == stamp
                && asset.authored_path == authored_path
                && asset.document_kind == document_kind
        }) {
            items.push(known.clone());
            continue;
        }
        let kind = document_kind.clone().unwrap_or_else(|| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("file")
                .to_ascii_lowercase()
        });
        let document = document_kind
            .as_ref()
            .and_then(|_| std::fs::read_to_string(&path).ok())
            .and_then(|text| ara_json::parse(&text).ok());
        let id = if document_kind.is_some() {
            document
                .as_ref()
                .and_then(|value| value["id"].as_str())
                .map(str::to_owned)
        } else {
            std::fs::read_to_string(PathBuf::from(format!("{}.import.json", path.display())))
                .ok()
                .and_then(|text| ara_json::parse(&text).ok())
                .and_then(|value| value["id"].as_str().map(str::to_owned))
        };
        let mut uses = Vec::new();
        if let Some(document) = &document {
            references(document, &mut uses);
        }
        let asset = Asset {
            path: entry.storage_path.clone(),
            reference: authored_path
                .as_ref()
                .zip(id)
                .map(|(path, id)| format!("@{path}#{id}")),
            domain: document
                .as_ref()
                .and_then(|value| value["records"]["g_meta"]["domain"].as_str())
                .map(str::to_owned),
            authored_path,
            document_kind,
            kind,
            stamp,
            uses,
        };
        cache.insert(entry.storage_path.clone(), asset.clone());
        items.push(asset);
    }
    let live: std::collections::HashSet<_> =
        items.iter().map(|asset| asset.path.as_str()).collect();
    cache.retain(|path, _| live.contains(path.as_str()));
    Ok(Snapshot { files, items })
}
fn references(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) if s.starts_with('@') => out.push(s.clone()),
        Value::Array(a) => {
            for v in a {
                references(v, out);
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                references(v, out);
            }
        }
        _ => {}
    }
}

fn stamp(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for path in [
        path.to_path_buf(),
        PathBuf::from(format!("{}.import.json", path.display())),
    ] {
        if let Ok(meta) = std::fs::metadata(path) {
            meta.len().hash(&mut hash);
            meta.modified().ok().hash(&mut hash);
        }
    }
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_identity_distinguishes_authored_and_project_files_with_the_same_logical_name() {
        let mut authored =
            Asset::unindexed("game/settings/builds/demo.json".into(), "export".into());
        authored.authored_path = Some("settings/builds/demo.json".into());
        authored.document_kind = Some("export".into());
        authored.reference = Some("@settings/builds/demo.json#ex_authored".into());
        let mut project = Asset::unindexed("settings/builds/demo.json".into(), "export".into());
        project.document_kind = Some("export".into());
        assert_ne!(
            ara_ui::source::stable_id(&authored.path),
            ara_ui::source::stable_id(&project.path)
        );
        let workspace = ara_doc::Workspace::default();
        assert_eq!(
            authored.current_reference(&workspace).as_deref(),
            Some("@settings/builds/demo.json#ex_authored")
        );
        assert!(project.current_reference(&workspace).is_none());
    }
    #[test]
    fn an_actual_legacy_game_folder_wins_over_the_modern_root_name() {
        let mut file = AuthoredFile {
            path: "game".into(),
            storage_path: "game".into(),
            scope: AuthoredScope::Authored,
            role: AuthoredRole::Folder,
        };
        assert_eq!(
            cached_authored_folder(std::slice::from_ref(&file), "", "game").as_deref(),
            Ok("game")
        );
        assert!(cached_authored_folder(&[], "", "game").is_err());
        assert_eq!(
            cached_authored_folder(&[], ara_doc::AUTHORING_ROOT, ara_doc::AUTHORING_ROOT)
                .as_deref(),
            Ok("")
        );
        file.scope = AuthoredScope::Project;
        assert!(cached_authored_folder(&[file], ara_doc::AUTHORING_ROOT, "game").is_err());
    }
}
