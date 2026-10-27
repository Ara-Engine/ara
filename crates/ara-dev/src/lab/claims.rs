//! Path-component ownership, for overlap warnings and compiler-error routing.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Claims(pub BTreeMap<String, Vec<PathBuf>>);

fn normalize(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path.replace('\\', "/"));
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|p| matches!(p, Component::Normal(_))),
        "claims must be workspace-relative paths without .. or wildcards"
    );
    ensure!(
        !path.to_string_lossy().contains('*'),
        "claim a directory instead of a glob"
    );
    Ok(path)
}

impl Claims {
    pub fn claim(&mut self, agent: &str, paths: &[String]) -> Result<Vec<String>> {
        ensure!(
            !agent.trim().is_empty() && !paths.is_empty(),
            "agent and paths are required"
        );
        let paths = paths
            .iter()
            .map(|p| normalize(p))
            .collect::<Result<Vec<_>>>()?;
        let warnings = self
            .0
            .iter()
            .filter(|(owner, _)| *owner != agent)
            .flat_map(|(owner, owned)| {
                paths
                    .iter()
                    .filter(move |path| {
                        owned
                            .iter()
                            .any(|other| path.starts_with(other) || other.starts_with(path))
                    })
                    .map(move |path| format!("{} overlaps {owner}", path.display()))
            })
            .collect();
        self.0.insert(agent.to_owned(), paths);
        Ok(warnings)
    }
    pub fn owners(&self, path: &Path) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(_, paths)| paths.iter().any(|p| path.starts_with(p)))
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlap_uses_components_and_routes_multiple_owners() {
        let mut claims = Claims::default();
        claims.claim("gi", &["crates/ara-gpu".into()]).unwrap();
        assert!(
            claims
                .claim("other", &["crates/ara-gpu2".into()])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            claims
                .claim("metal", &["crates/ara-gpu/src/metal".into()])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            claims.owners(Path::new("crates/ara-gpu/src/metal/reload.rs")),
            ["gi", "metal"]
        );
        assert!(claims.claim("bad", &["../x".into()]).is_err());
    }
}
