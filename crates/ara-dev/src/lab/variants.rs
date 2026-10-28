//! Published variant snapshots are separate from cooks and every resident player's view.
use super::{session, watch};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Recipe {
    pub packages: BTreeMap<PathBuf, Value>,
}
#[derive(Deserialize, Serialize)]
struct Definition {
    #[serde(default = "rebase")]
    rebase: bool,
    recipe: Recipe,
    #[serde(default)]
    pending: BTreeSet<PathBuf>,
    #[serde(default)]
    results: BTreeMap<PathBuf, bool>,
    #[serde(default)]
    published: bool,
}
#[derive(Default, Deserialize, Serialize)]
pub(super) struct Variants(BTreeMap<String, Definition>);
fn rebase() -> bool {
    true
}

impl Variants {
    pub fn load(base: &Path) -> Self {
        session::read(&base.join("variants.json"))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, base: &Path) -> Result<()> {
        session::write(&base.join("variants.json"), self)
    }
    pub fn remove(&mut self, base: &Path, name: &str) -> Result<()> {
        ensure!(name != "baseline", "baseline is reserved");
        self.0.remove(name).context("unknown variant")?;
        self.save(base)
    }
    pub fn status(&self) -> Value {
        json!(self)
    }
    pub fn register(
        &mut self,
        base: &Path,
        name: &str,
        recipe: Value,
        manifests: &[PathBuf],
        root: &Path,
    ) -> Result<Vec<PathBuf>> {
        session::name(name)?;
        ensure!(name != "baseline", "baseline is reserved");
        let recipe: Recipe = serde_json::from_value(recipe)?;
        ensure!(!recipe.packages.is_empty(), "variant recipe needs packages");
        let mut paths = Vec::new();
        for (manifest, value) in &recipe.packages {
            ensure!(
                manifest.is_relative()
                    && !manifest
                        .components()
                        .any(|p| !matches!(p, std::path::Component::Normal(_))),
                "variant manifests must be workspace-relative"
            );
            let path = root.join(manifest);
            ensure!(
                manifests.contains(&path),
                "{} is not a watched shader package",
                manifest.display()
            );
            ensure!(
                value["pipelines"]
                    .as_object()
                    .is_some_and(|p| !p.is_empty()),
                "package override needs pipelines"
            );
            paths.push(path);
        }
        let published = self.0.get(name).is_some_and(|v| v.published);
        self.0.insert(
            name.into(),
            Definition {
                rebase: true,
                recipe,
                pending: Default::default(),
                results: Default::default(),
                published,
            },
        );
        self.save(base)?;
        Ok(paths)
    }
    pub fn affected(&self, relative: &Path) -> Vec<String> {
        self.0
            .iter()
            .filter(|(_, v)| v.recipe.packages.contains_key(relative))
            .map(|(k, _)| k.clone())
            .collect()
    }
    pub fn queued(&mut self, name: &str, relative: &Path) {
        if let Some(v) = self.0.get_mut(name) {
            v.pending.insert(relative.into());
            v.results.remove(relative);
        }
    }
    pub fn recipe_path(&self, base: &Path, name: &str, relative: &Path) -> Result<PathBuf> {
        let value = &self
            .0
            .get(name)
            .context("unknown variant")?
            .recipe
            .packages
            .get(relative)
            .context("variant package")?;
        let path = base
            .join("variants")
            .join(name)
            .join("recipes")
            .join(relative);
        fs::create_dir_all(path.parent().context("recipe parent")?)?;
        session::write(&path, value)?;
        Ok(path)
    }
    pub fn completed(
        &mut self,
        base: &Path,
        name: &str,
        relative: &Path,
        success: bool,
    ) -> Result<bool> {
        let Some(v) = self.0.get_mut(name) else {
            return Ok(false);
        };
        v.pending.remove(relative);
        v.results.insert(relative.into(), success);
        let ready = v.pending.is_empty()
            && v.results.len() == v.recipe.packages.len()
            && v.results.values().all(|v| *v);
        self.save(base)?;
        Ok(ready)
    }
    pub fn ready(&self, name: &str) -> Result<()> {
        if name == "baseline" {
            return Ok(());
        }
        let v = self
            .0
            .get(name)
            .context("unknown variant; supply a recipe first")?;
        ensure!(
            v.published && v.pending.is_empty() && v.results.values().all(|v| *v),
            "variant is cooking or has errors; inspect lab_status"
        );
        Ok(())
    }
    pub fn publish(&mut self, root: &Path, base: &Path, name: &str) -> Result<bool> {
        let v = self.0.get_mut(name).context("unknown variant")?;
        let destination = profile(base, name);
        let rebased = !v.published || v.rebase;
        if rebased {
            publish_tree(&profile(base, "baseline"), &destination)?;
        }
        for manifest in v.recipe.packages.keys() {
            let source = watch::output(root, &cache(base, name), &root.join(manifest))?;
            let relative = source.strip_prefix(cache(base, name).join("shaders"))?;
            publish_tree(&source, &destination.join(relative))?;
        }
        v.published = true;
        v.rebase = false;
        self.save(base)?;
        Ok(rebased)
    }
    pub fn baseline_changed(
        &self,
        base: &Path,
        manifest: &Path,
        relative_output: &Path,
    ) -> Result<()> {
        for (name, v) in &self.0 {
            if v.published && !v.recipe.packages.contains_key(manifest) {
                publish_tree(
                    &profile(base, "baseline").join(relative_output),
                    &profile(base, name).join(relative_output),
                )?;
            }
        }
        Ok(())
    }
    pub fn packages(&self, name: &str) -> Result<Vec<PathBuf>> {
        Ok(self
            .0
            .get(name)
            .context("unknown variant")?
            .recipe
            .packages
            .keys()
            .cloned()
            .collect())
    }
    pub fn outstanding(&self) -> Vec<(String, PathBuf)> {
        self.0
            .iter()
            .flat_map(|(name, v)| {
                v.recipe
                    .packages
                    .keys()
                    .filter(|path| v.pending.contains(*path) || v.results.get(*path) != Some(&true))
                    .map(|path| (name.clone(), path.clone()))
            })
            .collect()
    }
}
pub(super) fn cache(base: &Path, name: &str) -> PathBuf {
    base.join("cook-cache").join(name)
}
pub(super) fn profile(base: &Path, name: &str) -> PathBuf {
    if name == "baseline" {
        base.join("shaders")
    } else {
        base.join("variants").join(name).join("shaders")
    }
}
pub(super) fn view(base: &Path, name: &str) -> PathBuf {
    base.join("views").join(name).join("shaders")
}

/// Existing views need only the cohorts whose publication token differs. A fresh
/// view and legacy cohorts without tokens still use the complete-copy contract.
pub(super) fn publish_selection(source: &Path, destination: &Path) -> Result<()> {
    if !destination.is_dir() {
        return publish_tree(source, destination);
    }
    if let Ok(token) = fs::read(source.join(".ara-shader-reload")) {
        return if fs::read(destination.join(".ara-shader-reload"))
            .ok()
            .as_ref()
            == Some(&token)
        {
            Ok(())
        } else {
            publish_tree(source, destination)
        };
    }
    let entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    if entries.iter().any(|e| {
        let path = e.path();
        path.extension().is_some_and(|x| x == "json")
            && !e.file_name().to_string_lossy().starts_with('.')
    }) {
        let unchanged = entries.iter().all(|entry| {
            entry.file_type().is_ok_and(|ty| {
                ty.is_dir()
                    || entry.metadata().is_ok_and(|source| {
                        fs::metadata(destination.join(entry.file_name())).is_ok_and(|target| {
                            source.len() == target.len()
                                && source.modified().ok() == target.modified().ok()
                        })
                    })
            })
        });
        if unchanged {
            return Ok(());
        }
        return publish_tree(source, destination);
    }
    for entry in entries {
        let ty = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            publish_selection(&entry.path(), &target)?;
        } else {
            publish_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Copy immutable artifacts before metadata and the cohort signal. Preserve mtimes
/// so a warm publication only copies changed files. Never modify a tracked output.
pub(super) fn publish_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| {
        let p = e.path();
        if p.is_dir() {
            0
        } else if p.file_name().is_some_and(|n| n == ".ara-shader-reload") {
            3
        } else if p.extension().is_some_and(|e| e == "json") {
            2
        } else {
            1
        }
    });
    for entry in entries {
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        let target = destination.join(entry.file_name());
        if ty.is_dir() {
            publish_tree(&entry.path(), &target)?;
            continue;
        }
        publish_file(&entry.path(), &target)?;
    }
    Ok(())
}

fn publish_file(source: &Path, target: &Path) -> Result<()> {
    let meta = fs::metadata(source)?;
    if fs::metadata(target)
        .is_ok_and(|old| old.len() == meta.len() && old.modified().ok() == meta.modified().ok())
    {
        return Ok(());
    }
    let temporary = target.with_extension(format!("{}.tmp", session::id()));
    fs::copy(source, &temporary)?;
    fs::File::options()
        .write(true)
        .open(&temporary)?
        .set_times(fs::FileTimes::new().set_modified(meta.modified()?))?;
    fs::rename(temporary, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_switches_and_restores_signalled_and_legacy_cohorts() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/lab-tests")
            .join(session::id());
        let baseline = root.join("baseline");
        for cohort in ["a", "b"] {
            fs::create_dir_all(baseline.join(cohort).join("key"))?;
            fs::write(baseline.join(cohort).join("pipeline.json"), b"baseline")?;
            fs::write(baseline.join(cohort).join("key/shader"), b"baseline")?;
        }
        fs::write(baseline.join("a/.ara-shader-reload"), b"baseline")?;
        let experiment = root.join("experiment");
        let view = root.join("view");
        publish_tree(&baseline, &experiment)?;
        publish_tree(&baseline, &view)?;
        for cohort in ["a", "b"] {
            fs::write(experiment.join(cohort).join("pipeline.json"), b"experiment")?;
            fs::write(experiment.join(cohort).join("key/shader"), b"experiment")?;
        }
        fs::write(experiment.join("a/.ara-shader-reload"), b"experiment")?;
        publish_selection(&experiment, &view)?;
        for cohort in ["a", "b"] {
            assert_eq!(
                fs::read(view.join(cohort).join("key/shader"))?,
                b"experiment"
            );
        }
        publish_selection(&baseline, &view)?;
        for cohort in ["a", "b"] {
            assert_eq!(fs::read(view.join(cohort).join("key/shader"))?, b"baseline");
        }
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn changed_recipe_rebases_removed_overrides_before_publication() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/lab-tests")
            .join(session::id());
        let base = root.join("lab");
        fs::create_dir_all(base.join("shaders/crates/a/cooked"))?;
        let manifest = root.join("crates/a/shaders/pipelines.json");
        fs::create_dir_all(manifest.parent().context("manifest parent")?)?;
        fs::write(&manifest, b"{}")?;
        fs::write(base.join("shaders/crates/a/cooked/a.json"), b"baseline")?;
        let other = root.join("crates/b/shaders/pipelines.json");
        fs::create_dir_all(other.parent().context("other parent")?)?;
        fs::write(&other, b"{}")?;
        fs::create_dir_all(base.join("shaders/crates/b/cooked"))?;
        fs::write(base.join("shaders/crates/b/cooked/b.json"), b"baseline-b")?;
        let manifests = [manifest.clone(), other.clone()];
        let mut variants = Variants::default();
        let recipe = json!({"packages":{"crates/a/shaders/pipelines.json":{"pipelines":{"a":{"defines":{"TEST":"1"}}}},"crates/b/shaders/pipelines.json":{"pipelines":{"b":{"defines":{"TEST":"1"}}}}}});
        variants.register(&base, "test", recipe, &manifests, &root)?;
        for (manifest, file) in [(&manifest, "a.json"), (&other, "b.json")] {
            let output = watch::output(&root, &cache(&base, "test"), manifest)?;
            fs::create_dir_all(&output)?;
            fs::write(output.join(file), b"variant")?;
        }
        variants.publish(&root, &base, "test")?;
        let recipe = json!({"packages":{"crates/a/shaders/pipelines.json":{"pipelines":{"a":{"defines":{"TEST":"2"}}}}}});
        variants.register(&base, "test", recipe, &manifests, &root)?;
        variants.publish(&root, &base, "test")?;
        assert_eq!(
            fs::read(profile(&base, "test").join("crates/a/cooked/a.json"))?,
            b"variant"
        );
        assert_eq!(
            fs::read(profile(&base, "test").join("crates/b/cooked/b.json"))?,
            b"baseline-b"
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
