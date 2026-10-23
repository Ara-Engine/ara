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
    pub uses: Vec<String>,
}
pub(super) struct Browser {
    pub items: Vec<Asset>,
    pending: Receiver<Vec<Asset>>,
}
impl Browser {
    pub fn new(root: &Path) -> Self {
        let root = root.to_owned();
        let (tx, pending) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let mut files = Vec::new();
                walk(&root, &root, &mut files);
                if tx.send(files).is_err() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
        Self {
            items: Vec::new(),
            pending,
        }
    }
    pub fn poll(&mut self) -> bool {
        if let Ok(items) = self.pending.try_recv() {
            let changed = self.items != items;
            self.items = items;
            changed
        } else {
            false
        }
    }
}
fn walk(root: &Path, dir: &Path, out: &mut Vec<Asset>) {
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
            walk(root, &path, out);
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
            let id = if kind == "scene" || kind == "prefab" {
                std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| ara_json::parse(&s).ok())
                    .and_then(|v| v["id"].as_str().map(str::to_owned))
            } else {
                let sidecar = PathBuf::from(format!("{}.import.json", path.display()));
                std::fs::read_to_string(sidecar)
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v["id"].as_str().map(str::to_owned))
            };
            out.push(Asset {
                reference: id.map(|id| format!("@{relative}#{id}")),
                path: relative,
                kind,
                uses: std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| ara_json::parse(&s).ok())
                    .map(|v| {
                        let mut refs = Vec::new();
                        references(&v, &mut refs);
                        refs
                    })
                    .unwrap_or_default(),
            });
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
