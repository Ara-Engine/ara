//! Publication order is explicit and testable without GitHub or signing secrets.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Prepared {
    pub format: String,
    pub version: String,
    pub repo: String,
    pub source_head: String,
    pub version_commit: String,
    pub source: PathBuf,
    pub target_dir: PathBuf,
    pub tool: PathBuf,
    pub tool_hash: String,
    pub artifacts: PathBuf,
    pub smoke_passed: bool,
    pub hub_published: bool,
    pub engine_published: bool,
}

pub(super) fn repository(repo: &str) -> Result<()> {
    ensure!(
        repo.split('/').count() == 2
            && repo.split('/').all(|part| !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))),
        "release repository must be owner/name"
    );
    Ok(())
}

/// Every publish invocation remains a Preview-channel GitHub prerelease. Signed
/// local folders are prepared first; the flag can only change the upload step.
pub(super) fn publish_args(prepared: &Prepared, kind: &str, publish: bool) -> Vec<String> {
    let mut args = vec![
        "publish".into(),
        "--repo".into(),
        prepared.repo.clone(),
        "--tag".into(),
        format!("{kind}-v{}", prepared.version),
        "--prerelease".into(),
        "--require-trusted-key".into(),
        "--artifacts".into(),
        prepared.artifacts.to_string_lossy().into_owned(),
    ];
    if !publish {
        args.push("--dry-run".into());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_and_uploaded_previews_keep_identical_tags_and_channels() {
        let prepared = Prepared {
            format: "ara.preview/1".into(),
            version: "0.0.2-alpha.1".into(),
            repo: "Ara-Engine/ara".into(),
            source_head: String::new(),
            version_commit: String::new(),
            source: PathBuf::new(),
            target_dir: PathBuf::new(),
            tool: PathBuf::new(),
            tool_hash: String::new(),
            artifacts: "artifacts".into(),
            smoke_passed: true,
            hub_published: false,
            engine_published: false,
        };
        for kind in ["hub", "engine"] {
            let mut local = publish_args(&prepared, kind, false);
            assert_eq!(local.pop().as_deref(), Some("--dry-run"));
            assert_eq!(local, publish_args(&prepared, kind, true));
            assert!(local.iter().any(|arg| arg == "--prerelease"));
            assert!(
                local
                    .iter()
                    .any(|arg| arg == &format!("{kind}-v0.0.2-alpha.1"))
            );
        }
        for repo in ["Ara-Engine/ara", "private/staging"] {
            repository(repo).unwrap();
        }
        for repo in [
            "../escape",
            "-bad/repo extra",
            "https://github.com/owner/repo",
            "owner/repo/more",
        ] {
            assert!(repository(repo).is_err());
        }
    }
}
