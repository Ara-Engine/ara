use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Asset {
    pub path: String,
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
        workspace
            .index()
            .id_of(&self.path)
            .map(|id| format!("@{}#{id}", self.path))
            .or_else(|| self.reference.clone())
    }
}
pub(super) struct Browser {
    pub items: Vec<Asset>,
    /// Bumped whenever `items` changes (memoized panels list them).
    pub revision: u64,
    pending: Receiver<Vec<Asset>>,
}
impl Browser {
    pub fn new(root: &Path) -> Self {
        let root = root.to_owned();
        let (tx, pending) = mpsc::channel();
        std::thread::spawn(move || {
            // Parsed documents are reused while a file's stamp is unchanged, so a rescan
            // only stats binary media instead of reading it.
            let mut cache = std::collections::HashMap::new();
            loop {
                let mut files = Vec::new();
                walk(&root, &root, &mut files, &mut cache);
                cache.retain(|path: &String, _| files.iter().any(|a| a.path == *path));
                if tx.send(files).is_err() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
        Self {
            items: Vec::new(),
            revision: 0,
            pending,
        }
    }
    pub fn poll(&mut self) -> bool {
        if let Ok(items) = self.pending.try_recv() {
            let changed = self.items != items;
            self.items = items;
            self.revision += u64::from(changed);
            changed
        } else {
            false
        }
    }
}
/// JSON documents whose ID, domain and references the browser reads.
const DOCUMENTS: &[&str] = &[
    "scene",
    "prefab",
    "graph",
    "animation",
    "materialinstance",
    "material",
    "terrain",
    "foliage",
    "world",
    "music",
];
fn walk(
    root: &Path,
    dir: &Path,
    out: &mut Vec<Asset>,
    cache: &mut std::collections::HashMap<String, Asset>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules" | "spikes") {
            continue;
        }
        let Ok(ty) = entry.file_type() else {
            continue;
        };
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            if path != root && path.join("ara.project.json").exists() {
                continue;
            }
            walk(root, &path, out, cache);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let kind = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("file")
                .to_owned();
            if relative.ends_with(".import.json") {
                continue;
            }
            let stamp = stamp(&path);
            if let Some(known) = cache.get(&relative).filter(|a| a.stamp == stamp) {
                out.push(known.clone());
                continue;
            }
            let document = DOCUMENTS
                .contains(&kind.as_str())
                .then(|| std::fs::read_to_string(&path).ok())
                .flatten()
                .and_then(|text| ara_json::parse(&text).ok());
            let domain = document
                .as_ref()
                .and_then(|v| v["records"]["g_meta"]["domain"].as_str())
                .map(str::to_owned);
            let id = if DOCUMENTS.contains(&kind.as_str()) {
                document
                    .as_ref()
                    .and_then(|v| v["id"].as_str().map(str::to_owned))
            } else {
                let sidecar = PathBuf::from(format!("{}.import.json", path.display()));
                std::fs::read_to_string(sidecar)
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v["id"].as_str().map(str::to_owned))
            };
            let asset = Asset {
                reference: id.map(|id| format!("@{relative}#{id}")),
                stamp,
                path: relative.clone(),
                kind,
                domain,
                uses: document
                    .map(|v| {
                        let mut refs = Vec::new();
                        references(&v, &mut refs);
                        refs
                    })
                    .unwrap_or_default(),
            };
            cache.insert(relative, asset.clone());
            out.push(asset);
        }
    }
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
