//! Verification output stays in Cargo scratch space unless publication is explicit.
use anyhow::{Result, ensure};
use std::path::{Component, Path, PathBuf};

/// Locate a CLI measurement or capture. Only `ARA_UPDATE_MEASUREMENTS=1` permits
/// writing beneath the crate's checked-in artifact directories.
pub fn output_path(root: &Path, relative: impl AsRef<Path>) -> Result<PathBuf> {
    prepare(destination(
        root,
        relative.as_ref(),
        std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from),
        std::env::var("ARA_UPDATE_MEASUREMENTS").as_deref() == Ok("1"),
    )?)
}

/// Locate test/diagnostic output in scratch space, ignoring publication opt-ins.
/// Defaults to workspace `target/ara-pathtrace`, or uses `CARGO_TARGET_TMPDIR`.
pub fn temporary_path(root: &Path, relative: impl AsRef<Path>) -> Result<PathBuf> {
    prepare(destination(
        root,
        relative.as_ref(),
        std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from),
        false,
    )?)
}

fn destination(
    root: &Path,
    relative: &Path,
    temporary: Option<PathBuf>,
    publish: bool,
) -> Result<PathBuf> {
    ensure!(
        relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
            && !relative.as_os_str().is_empty(),
        "Artifact names must be relative paths without parent traversal"
    );
    let base = if publish {
        root.to_path_buf()
    } else {
        temporary
            .unwrap_or_else(|| root.join("../../target"))
            .join("ara-pathtrace")
    };
    Ok(base.join(relative))
}

fn prepare(path: PathBuf) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurements_and_captures_need_explicit_publication() -> Result<()> {
        let root = Path::new("workspace/crates/ara-pathtrace");
        for name in ["measurements/quality-targets.json", "captures/night.png"] {
            let name = Path::new(name);
            assert_eq!(
                destination(root, name, None, false)?,
                root.join("../../target/ara-pathtrace").join(name)
            );
            assert_eq!(
                destination(root, name, Some("cargo-tmp".into()), false)?,
                Path::new("cargo-tmp/ara-pathtrace").join(name)
            );
            assert_eq!(
                destination(root, name, Some("cargo-tmp".into()), true)?,
                root.join(name)
            );
        }
        assert!(destination(root, Path::new("../quality.json"), None, false).is_err());
        Ok(())
    }
}
