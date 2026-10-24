//! Workspace-only metadata includes optional, build and test dependency edges.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub(crate) struct Metadata {
    pub(crate) packages: Vec<Package>,
    pub(crate) target_directory: PathBuf,
}

#[derive(Deserialize)]
pub(crate) struct Package {
    pub(crate) name: String,
    pub(crate) manifest_path: PathBuf,
    pub(crate) dependencies: Vec<Dependency>,
    #[serde(default)]
    pub(crate) targets: Vec<Target>,
}

#[derive(Deserialize)]
pub(crate) struct Target {
    #[serde(default)]
    pub(crate) name: String,
    pub(crate) kind: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct Dependency {
    pub(crate) name: String,
    path: Option<PathBuf>,
}

impl Metadata {
    pub(crate) fn owner(&self, root: &Path, path: &Path) -> Option<&Package> {
        self.packages
            .iter()
            .filter(|p| {
                p.manifest_path
                    .parent()
                    .is_some_and(|dir| root.join(path).starts_with(dir))
            })
            .max_by_key(|p| p.manifest_path.components().count())
    }

    pub(crate) fn reverse_closure(&self, roots: &BTreeSet<String>) -> BTreeSet<String> {
        let mut affected = roots.clone();
        let by_name: BTreeMap<_, _> = self.packages.iter().map(|p| (p.name.as_str(), p)).collect();
        loop {
            let before = affected.len();
            for p in &self.packages {
                if p.dependencies.iter().any(|d| {
                    d.path.is_some()
                        && by_name.contains_key(d.name.as_str())
                        && affected.contains(&d.name)
                }) {
                    affected.insert(p.name.clone());
                }
            }
            if affected.len() == before {
                break;
            }
        }
        affected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> Metadata {
        let root = std::env::current_dir().unwrap();
        Metadata {
            target_directory: root.join("target"),
            packages: [
                ("core", vec![]),
                ("renderer", vec!["core"]),
                ("editor", vec!["renderer"]),
                ("isolated", vec![]),
            ]
            .into_iter()
            .map(|(name, dependencies)| Package {
                name: name.into(),
                targets: Vec::new(),
                manifest_path: root.join(format!("crates/{name}/Cargo.toml")),
                dependencies: dependencies
                    .into_iter()
                    .map(|name| Dependency {
                        name: name.into(),
                        path: Some(root.join(format!("crates/{name}"))),
                    })
                    .collect(),
            })
            .collect(),
        }
    }

    #[test]
    fn affected_is_transitive_including_test_build_optional_and_renamed_edges() {
        let m = metadata();
        assert_eq!(
            m.reverse_closure(&BTreeSet::from(["core".into()])),
            BTreeSet::from(["core".into(), "renderer".into(), "editor".into()])
        );
        assert!(m.reverse_closure(&BTreeSet::new()).is_empty());
    }
}
