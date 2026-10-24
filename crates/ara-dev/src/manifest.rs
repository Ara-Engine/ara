//! Compare resolved dependencies, not the spelling of workspace inheritance.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use anyhow::{Context, Result, ensure};
use semver::{Version, VersionReq};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PackageId {
    pub(crate) name: String,
    version: String,
    source: Option<String>,
}

#[derive(Deserialize)]
struct LockedPackage {
    #[serde(flatten)]
    id: PackageId,
    checksum: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Default, Deserialize)]
pub(crate) struct Lock {
    #[serde(default, rename = "package")]
    packages: Vec<LockedPackage>,
}

impl Lock {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        toml_edit::de::from_str(text).context("parse Cargo.lock")
    }

    fn lookup(&self, dependency: &str) -> Result<&LockedPackage> {
        let mut words = dependency.split_whitespace();
        let name = words.next().context("empty lock dependency")?;
        let version = words.next();
        let source = words.next().map(|s| s.trim_matches(['(', ')']));
        let candidates: Vec<_> = self
            .packages
            .iter()
            .filter(|p| {
                p.id.name == name
                    && version.is_none_or(|v| p.id.version == v)
                    && source.is_none_or(|s| p.id.source.as_deref() == Some(s))
            })
            .collect();
        ensure!(
            candidates.len() == 1,
            "ambiguous lock dependency {dependency}"
        );
        Ok(candidates[0])
    }

    fn resolve(
        &self,
        owner: &str,
        name: &str,
        requirement: Option<&str>,
    ) -> Result<Vec<PackageId>> {
        let req = requirement.map(VersionReq::parse).transpose()?;
        let mut resolved = Vec::new();
        for package in self
            .packages
            .iter()
            .filter(|p| p.id.name == owner && p.id.source.is_none())
        {
            for dependency in &package.dependencies {
                let candidate = self.lookup(dependency)?;
                if candidate.id.name == name
                    && req.as_ref().is_none_or(|r| {
                        Version::parse(&candidate.id.version).is_ok_and(|v| r.matches(&v))
                    })
                {
                    resolved.push(candidate.id.clone());
                }
            }
        }
        resolved.sort();
        resolved.dedup();
        ensure!(
            !resolved.is_empty(),
            "{owner}'s {name} dependency is absent from Cargo.lock"
        );
        Ok(resolved)
    }

    /// Optional feature edges can grow without changing another crate's resolution.
    /// Replacements, removals of resolved versions and checksum changes are real updates.
    pub(crate) fn changed_external(&self, current: &Self) -> Result<BTreeSet<PackageId>> {
        let mut changed = BTreeSet::new();
        for old in self.packages.iter().filter(|p| p.id.source.is_some()) {
            let Some(new) = current.packages.iter().find(|p| p.id == old.id) else {
                changed.insert(old.id.clone());
                continue;
            };
            if old.checksum != new.checksum {
                changed.insert(old.id.clone());
            }
            let old_edges = self.edges(old)?;
            let new_edges = current.edges(new)?;
            for (name, previous) in old_edges {
                if let Some(next) = new_edges.get(&name)
                    && !previous.is_subset(next)
                    && !next.is_subset(&previous)
                {
                    changed.insert(old.id.clone());
                }
            }
        }
        Ok(changed)
    }

    fn edges(&self, package: &LockedPackage) -> Result<BTreeMap<String, BTreeSet<PackageId>>> {
        let mut edges = BTreeMap::<_, BTreeSet<_>>::new();
        for dependency in &package.dependencies {
            let id = self.lookup(dependency)?.id.clone();
            edges.entry(id.name.clone()).or_default().insert(id);
        }
        Ok(edges)
    }

    pub(crate) fn reaches(&self, owner: &str, changed: &BTreeSet<PackageId>) -> Result<bool> {
        let mut pending = Vec::new();
        for package in self
            .packages
            .iter()
            .filter(|p| p.id.name == owner && p.id.source.is_none())
        {
            pending.extend(
                package
                    .dependencies
                    .iter()
                    .map(|d| self.lookup(d))
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        let mut visited = BTreeSet::new();
        while let Some(package) = pending.pop() {
            // Workspace edges are propagated separately through the metadata graph.
            if package.id.source.is_none() || !visited.insert(package.id.clone()) {
                continue;
            }
            if changed.contains(&package.id) {
                return Ok(true);
            }
            pending.extend(
                package
                    .dependencies
                    .iter()
                    .map(|d| self.lookup(d))
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        Ok(false)
    }
}

pub(crate) fn parse(text: &str) -> Result<Value> {
    toml_edit::de::from_str(text).context("parse Cargo manifest")
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Dependency {
    package: String,
    resolution: Vec<PackageId>,
    path: Option<String>,
    pub(crate) kind: String,
    target: String,
    features: BTreeSet<String>,
    default_features: bool,
    optional: bool,
    extra: String,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Manifest {
    shape: Value,
    dependencies: BTreeSet<Dependency>,
    pub(crate) bindings: BTreeMap<String, BTreeSet<Dependency>>,
}

impl Manifest {
    pub(crate) fn read(
        text: &str,
        workspace: &Value,
        lock: &Lock,
        directory: &Path,
    ) -> Result<Self> {
        let mut shape = parse(text)?;
        let package = shape
            .get_mut("package")
            .and_then(Value::as_object_mut)
            .context("package table")?;
        for (name, value) in package.iter_mut() {
            if value.get("workspace") == Some(&Value::Bool(true)) {
                *value = workspace["workspace"]["package"][name].clone();
            }
        }
        let owner = package
            .get("name")
            .and_then(Value::as_str)
            .context("package name")?
            .to_owned();
        let mut result = Self {
            shape: Value::Null,
            dependencies: BTreeSet::new(),
            bindings: BTreeMap::new(),
        };
        result.take_dependencies(&mut shape, workspace, lock, directory, &owner, "")?;
        if let Some(targets) = shape.get_mut("target").and_then(Value::as_object_mut) {
            for (target, table) in targets.iter_mut() {
                result.take_dependencies(table, workspace, lock, directory, &owner, target)?;
            }
            targets.retain(|_, v| v.as_object().is_none_or(|v| !v.is_empty()));
            if targets.is_empty() {
                shape
                    .as_object_mut()
                    .context("manifest table")?
                    .remove("target");
            }
        }
        if let Some(features) = shape.get_mut("features").and_then(Value::as_object_mut) {
            for values in features.values_mut().filter_map(Value::as_array_mut) {
                // Dependency aliases in feature declarations keep their local names;
                // changing one is conservatively a manifest change.
                values.sort_by_key(Value::to_string);
                values.dedup();
            }
        }
        result.shape = shape;
        Ok(result)
    }

    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        self.shape == other.shape && self.dependencies == other.dependencies
    }

    fn take_dependencies(
        &mut self,
        shape: &mut Value,
        workspace: &Value,
        lock: &Lock,
        directory: &Path,
        owner: &str,
        target: &str,
    ) -> Result<()> {
        for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let Some(declarations) = shape.as_object_mut().and_then(|m| m.remove(kind)) else {
                continue;
            };
            for (local, declaration) in declarations.as_object().context("dependency table")? {
                let inherited = declaration.get("workspace") == Some(&Value::Bool(true));
                let mut dep = table(if inherited {
                    &workspace["workspace"]["dependencies"][local]
                } else {
                    declaration
                })?;
                let mut features = strings(dep.get("features"));
                if inherited {
                    features.extend(strings(declaration.get("features")));
                    for field in ["optional", "default-features"] {
                        if let Some(value) = declaration.get(field) {
                            dep.insert(field.into(), value.clone());
                        }
                    }
                }
                let name = dep
                    .get("package")
                    .and_then(Value::as_str)
                    .unwrap_or(local)
                    .to_owned();
                let path = dep.get("path").and_then(Value::as_str).map(|p| {
                    relative(&(if inherited { Path::new("") } else { directory }).join(p))
                });
                let dependency = Dependency {
                    resolution: lock.resolve(
                        owner,
                        &name,
                        dep.get("version").and_then(Value::as_str),
                    )?,
                    package: name,
                    path,
                    kind: kind.into(),
                    target: target.into(),
                    features,
                    default_features: dep
                        .get("default-features")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    optional: dep
                        .get("optional")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    extra: serde_json::to_string(
                        &dep.iter()
                            .filter(|(key, _)| {
                                ![
                                    "version",
                                    "package",
                                    "path",
                                    "features",
                                    "default-features",
                                    "optional",
                                    "workspace",
                                ]
                                .contains(&key.as_str())
                            })
                            .collect::<BTreeMap<_, _>>(),
                    )?,
                };
                self.dependencies.insert(dependency.clone());
                self.bindings
                    .entry(local.replace('-', "_"))
                    .or_default()
                    .insert(dependency);
            }
        }
        Ok(())
    }
}

fn table(value: &Value) -> Result<Map<String, Value>> {
    if let Some(version) = value.as_str() {
        Ok(Map::from_iter([(
            "version".into(),
            Value::String(version.into()),
        )]))
    } else {
        value.as_object().cloned().context("dependency declaration")
    }
}

fn strings(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn relative(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if parts.last().is_some_and(|p| p != "..") {
                    parts.pop();
                } else {
                    parts.push("..".to_owned());
                }
            }
            Component::CurDir => {}
            other => parts.push(other.as_os_str().to_string_lossy().into_owned()),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
[[package]]
name = "app"
version = "0.1.0"
dependencies = ["serde"]
[[package]]
name = "serde"
version = "1.0.229"
source = "registry+example"
"#;

    #[test]
    fn workspace_moves_preserve_resolved_versions_features_and_dependency_kinds() {
        let old_root =
            parse("[workspace.dependencies]\nserde = { version = \"1\", features = [\"derive\"] }")
                .unwrap();
        let current_root = parse("[workspace.dependencies]\nserde = \"1.0.229\"").unwrap();
        let header = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
        let lock = Lock::parse(LOCK).unwrap();
        let old = Manifest::read(
            &format!("{header}[dependencies]\nserde.workspace = true"),
            &old_root,
            &lock,
            Path::new("app"),
        )
        .unwrap();
        let current_text = format!(
            "{header}[dependencies]\nserde = {{ workspace = true, features = [\"derive\"] }}"
        );
        let current =
            Manifest::read(&current_text, &current_root, &lock, Path::new("app")).unwrap();
        assert!(old.equivalent(&current));
        let no_features = Manifest::read(
            &format!("{header}[dependencies]\nserde.workspace = true"),
            &current_root,
            &lock,
            Path::new("app"),
        )
        .unwrap();
        assert!(!old.equivalent(&no_features));
        let build = Manifest::read(
            &current_text.replace(
                "[dependencies]",
                "[target.'cfg(windows)'.build-dependencies]",
            ),
            &current_root,
            &lock,
            Path::new("app"),
        )
        .unwrap();
        assert!(!old.equivalent(&build));
        let updated_lock = Lock::parse(&LOCK.replace("1.0.229", "1.0.230")).unwrap();
        let updated = Manifest::read(
            &current_text,
            &current_root,
            &updated_lock,
            Path::new("app"),
        )
        .unwrap();
        assert!(!old.equivalent(&updated));
    }

    #[test]
    fn lock_updates_invalidate_only_users_of_changed_resolutions() {
        let old_text = r#"
[[package]]
name = "app"
version = "0.1.0"
dependencies = ["codec"]
[[package]]
name = "other"
version = "0.1.0"
dependencies = ["leaf 2.0.0"]
[[package]]
name = "codec"
version = "1.0.0"
source = "registry+example"
dependencies = ["leaf 1.0.0"]
[[package]]
name = "leaf"
version = "1.0.0"
source = "registry+example"
[[package]]
name = "leaf"
version = "2.0.0"
source = "registry+example"
"#;
        let old = Lock::parse(old_text).unwrap();
        let new = Lock::parse(&old_text.replace(
            "dependencies = [\"leaf 1.0.0\"]",
            "dependencies = [\"leaf 2.0.0\"]",
        ))
        .unwrap();
        let changed = old.changed_external(&new).unwrap();
        assert!(old.reaches("app", &changed).unwrap());
        assert!(!old.reaches("other", &changed).unwrap());
        let only_workspace = Lock::parse(&format!(
            "{old_text}\n[[package]]\nname = \"new-tool\"\nversion = \"0.1.0\"\n"
        ))
        .unwrap();
        assert!(old.changed_external(&only_workspace).unwrap().is_empty());
        let optional_edge = Lock::parse(&old_text.replace(
            "dependencies = [\"leaf 1.0.0\"]",
            "dependencies = [\"leaf 1.0.0\", \"leaf 2.0.0\"]",
        ))
        .unwrap();
        assert!(old.changed_external(&optional_edge).unwrap().is_empty());
    }
}
