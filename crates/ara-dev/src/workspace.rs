//! Workspace-only metadata includes optional, build and test dependency edges.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

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
}

#[derive(Deserialize)]
pub(crate) struct Dependency {
    name: String,
    path: Option<PathBuf>,
}

impl Metadata {
    pub(crate) fn read(root: &Path) -> Result<Self> {
        let out = Command::new("cargo")
            .current_dir(root)
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .env("CARGO_BUILD_JOBS", "2")
            .output()
            .context("cargo metadata")?;
        ensure!(
            out.status.success(),
            "cargo metadata: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).context("parse cargo metadata")
    }

    /// Root build inputs invalidate everything. Documentation alone runs hygiene/fmt.
    pub(crate) fn affected(&self, root: &Path, changed: &[PathBuf]) -> BTreeSet<String> {
        let mut affected = BTreeSet::new();
        for path in changed {
            let owner = self
                .packages
                .iter()
                .filter(|p| {
                    p.manifest_path
                        .parent()
                        .is_some_and(|dir| root.join(path).starts_with(dir))
                })
                .max_by_key(|p| p.manifest_path.components().count());
            if let Some(owner) = owner {
                affected.insert(owner.name.clone());
            } else if !documentation(path) {
                return self.packages.iter().map(|p| p.name.clone()).collect();
            }
        }
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

fn documentation(path: &Path) -> bool {
    path.starts_with("docs")
        || path.starts_with("reports")
        || path.starts_with("spikes")
        || path.extension().is_some_and(|e| e == "md")
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
        let root = std::env::current_dir().unwrap();
        assert_eq!(
            m.affected(&root, &["crates/core/src/lib.rs".into()]),
            BTreeSet::from(["core".into(), "renderer".into(), "editor".into()])
        );
        assert_eq!(m.affected(&root, &["Cargo.lock".into()]).len(), 4);
        assert_eq!(m.affected(&root, &[".cargo/config.toml".into()]).len(), 4);
        assert_eq!(
            m.affected(&root, &["crates/deleted/src/lib.rs".into()])
                .len(),
            4
        );
        assert!(m.affected(&root, &["docs/guide.md".into()]).is_empty());
        assert_eq!(
            m.affected(&root, &["crates/core/AGENTS.md".into()]).len(),
            3
        );
    }
}
