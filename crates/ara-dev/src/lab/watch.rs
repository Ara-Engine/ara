//! Debounced source snapshots. Watch all manifests, including generated VFX cohorts.
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

type Stamp = (SystemTime, u64);
#[derive(Default)]
pub(super) struct Watch {
    files: BTreeMap<PathBuf, Stamp>,
    pending: BTreeMap<PathBuf, Stamp>,
    changed_at: Option<Instant>,
}
fn scan(path: &Path, files: &mut BTreeMap<PathBuf, Stamp>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            if ![
                "target",
                "cooked",
                ".git",
                ".cache",
                "node_modules",
                "bin",
                "obj",
            ]
            .iter()
            .any(|name| path.file_name().is_some_and(|f| f == *name))
            {
                scan(&path, files)?;
            }
        } else if path.extension().is_some_and(|e| {
            ["rs", "toml", "slang", "slangh", "h"]
                .iter()
                .any(|x| e == *x)
        }) || path.file_name().is_some_and(|f| f == "pipelines.json")
        {
            let meta = entry.metadata()?;
            files.insert(path, (meta.modified()?, meta.len()));
        }
    }
    Ok(())
}
impl Watch {
    pub fn poll(&mut self, root: &Path) -> Result<Vec<PathBuf>> {
        let mut current = BTreeMap::new();
        scan(&root.join("crates"), &mut current)?;
        scan(&root.join("shaders"), &mut current)?;
        for name in ["Cargo.toml", "Cargo.lock", ".cargo/config.toml"] {
            let path = root.join(name);
            let meta = fs::metadata(&path)?;
            current.insert(path, (meta.modified()?, meta.len()));
        }
        if self.files.is_empty() {
            self.files = current;
            return Ok(Vec::new());
        }
        if current == self.files {
            self.pending = current;
            self.changed_at = None;
            return Ok(Vec::new());
        }
        if self.pending != current {
            self.pending = current;
            self.changed_at = Some(Instant::now());
        }
        if self
            .changed_at
            .is_none_or(|t| t.elapsed() < Duration::from_millis(400))
        {
            return Ok(Vec::new());
        }
        let changed = self
            .files
            .keys()
            .chain(self.pending.keys())
            .filter(|path| self.files.get(*path) != self.pending.get(*path))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        self.files = std::mem::take(&mut self.pending);
        self.changed_at = None;
        Ok(changed)
    }
    pub fn manifests(&self) -> Vec<PathBuf> {
        self.files
            .keys()
            .filter(|p| p.file_name().is_some_and(|f| f == "pipelines.json"))
            .cloned()
            .collect()
    }
}

pub(super) fn output(root: &Path, base: &Path, manifest: &Path) -> Result<PathBuf> {
    let parent = manifest.parent().context("shader manifest parent")?;
    if parent == root.join("shaders") {
        return Ok(base.join("shaders/crates/ara-render/cooked"));
    }
    let relative = parent.strip_prefix(root)?;
    let destination = if parent
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|f| f == "generated")
    {
        relative
            .parent()
            .and_then(Path::parent)
            .context("generated package")?
            .join("cooked")
            .join(parent.file_name().context("effect name")?)
    } else if parent.file_name().is_some_and(|f| f == "shaders") {
        relative.parent().context("package")?.join("cooked")
    } else {
        relative.join("cooked")
    };
    Ok(base.join("shaders").join(destination))
}
pub(super) fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &path)?;
        } else {
            fs::copy(entry.path(), path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_all_cohorts_debounces_and_observes_deletions_without_cooked_feedback() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/lab-tests")
            .join(crate::lab::session::id());
        for name in [
            "crates/a/shaders/pipelines.json",
            "crates/a/generated/smoke/pipelines.json",
            "shaders/pipelines.json",
            "Cargo.toml",
            "Cargo.lock",
            ".cargo/config.toml",
            "crates/a/src/lib.rs",
        ] {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"initial").unwrap();
        }
        let mut watch = Watch::default();
        assert!(watch.poll(&root).unwrap().is_empty());
        assert_eq!(watch.manifests().len(), 3);
        fs::write(root.join("crates/a/src/lib.rs"), b"changed source").unwrap();
        assert!(watch.poll(&root).unwrap().is_empty());
        watch.changed_at = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(
            watch.poll(&root).unwrap(),
            [root.join("crates/a/src/lib.rs")]
        );
        fs::create_dir_all(root.join("crates/a/cooked")).unwrap();
        fs::write(root.join("crates/a/cooked/artifact.h"), b"ignored").unwrap();
        assert!(watch.poll(&root).unwrap().is_empty());
        fs::remove_file(root.join("crates/a/src/lib.rs")).unwrap();
        assert!(watch.poll(&root).unwrap().is_empty());
        watch.changed_at = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(
            watch.poll(&root).unwrap(),
            [root.join("crates/a/src/lib.rs")]
        );
    }
}
