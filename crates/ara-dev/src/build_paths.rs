//! Cargo outputs stay local unless an explicit, independent directory is selected.

use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

pub(crate) struct Directories {
    target: PathBuf,
    build: PathBuf,
}

impl Directories {
    pub(crate) fn from_env(root: &Path) -> Result<Self> {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .or_else(|| std::env::var_os("CARGO_BUILD_TARGET_DIR"))
            .filter(|value| !value.is_empty())
            .map_or_else(|| root.join("target"), PathBuf::from);
        let build = std::env::var_os("CARGO_BUILD_BUILD_DIR")
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var_os("ARA_SHARED_BUILD_DIR").filter(|v| !v.is_empty()))
            .map_or_else(|| root.join("target"), PathBuf::from);
        if let Some(shared) = std::env::var_os("ARA_SHARED_BUILD_DIR").filter(|v| !v.is_empty()) {
            ensure!(
                same_path(&resolve(root, Path::new(&shared))?, &resolve(root, &build)?),
                "ARA_SHARED_BUILD_DIR and CARGO_BUILD_BUILD_DIR disagree; use the shared directory for both so admission protects live artifacts"
            );
        }
        for (name, path) in [
            ("Cargo target directory", &target),
            ("Cargo build directory", &build),
        ] {
            ensure!(
                foreign_checkout(root, path)?.is_none(),
                "refusing {name} {} inside another checkout's target; use {} or an independent directory",
                path.display(),
                root.join("target").display()
            );
        }
        Ok(Self {
            target: resolve(root, &target)?,
            build: resolve(root, &build)?,
        })
    }

    pub(crate) fn apply(&self, command: &mut Command) {
        command
            .env("CARGO_TARGET_DIR", &self.target)
            .env("CARGO_BUILD_BUILD_DIR", &self.build);
    }
}

/// Resolve existing junctions and normalize missing suffixes without creating paths.
fn resolve(root: &Path, path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        let inspect = matches!(component, Component::Normal(_));
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
        // Windows cannot query reparse metadata for a verbatim drive prefix/root.
        // Junctions and symlinks can only occur in the normal path components.
        if inspect && resolved.is_absolute() {
            match fs::symlink_metadata(&resolved) {
                Ok(_) => {
                    resolved = resolved.canonicalize().with_context(|| {
                        format!("resolve Cargo output path {}", resolved.display())
                    })?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("inspect Cargo output path {}", resolved.display())
                    });
                }
            }
        }
    }
    Ok(resolved)
}

fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    } else {
        left == right
    }
}

/// Catch both a checkout's target itself and independent-looking caches nested in it.
fn foreign_checkout(root: &Path, path: &Path) -> Result<Option<PathBuf>> {
    let root = root.canonicalize()?;
    let path = resolve(&root, path)?;
    for ancestor in path.ancestors() {
        if same_path(ancestor, &root) {
            break;
        }
        let target = ancestor.file_name().is_some_and(|name| {
            if cfg!(windows) {
                name.to_string_lossy().eq_ignore_ascii_case("target")
            } else {
                name == "target"
            }
        });
        if target
            && let Some(checkout) = ancestor.parent()
            && checkout.join(".git").try_exists()?
        {
            if same_path(checkout, &root) {
                return Ok(None);
            }
            return Ok(Some(checkout.to_owned()));
        }
    }
    Ok(None)
}

pub(crate) fn validate_output(root: &Path, path: &Path) -> Result<()> {
    ensure!(
        foreign_checkout(root, path)?.is_none(),
        "refusing output {} inside another checkout's target; use a worktree-local or independent directory",
        path.display()
    );
    Ok(())
}

#[derive(Deserialize)]
struct Configured {
    target_directory: PathBuf,
    #[serde(default)]
    build_directory: Option<PathBuf>,
}

/// Inspect effective Cargo settings, including global config, without running a build.
pub(crate) fn disk_check(root: &Path) -> Result<bool> {
    let mut healthy = true;
    for name in [
        "ARA_SHARED_BUILD_DIR",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
    ] {
        if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            healthy &= report(root, name, Path::new(&value))?;
        }
    }
    // Do not create even diagnostic logs through a foreign target junction.
    if !report(root, "local target", &root.join("target"))? {
        return Ok(false);
    }
    if !healthy {
        return Ok(false);
    }
    let directory = root.join("target/ara-dev");
    fs::create_dir_all(&directory)?;
    let output = directory.join(format!("disk-cargo-{}.json", std::process::id()));
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--locked",
            "--offline",
        ])
        .env("CARGO_BUILD_JOBS", "2")
        .stdout(File::create(&output)?)
        .stderr(Stdio::null());
    let result = crate::process::run(&mut command, Some(Instant::now() + Duration::from_secs(10)));
    let bytes = fs::read(&output);
    let _ = fs::remove_file(output);
    if !result.as_ref().is_ok_and(crate::process::Outcome::success) {
        eprintln!("WARNING: Cargo output directories could not be inspected within 10 seconds");
        return Ok(false);
    }
    let configured: Configured = serde_json::from_slice(&bytes.context("read Cargo directories")?)?;
    healthy &= report(
        root,
        "configured Cargo target",
        &configured.target_directory,
    )?;
    if let Some(build) = configured.build_directory {
        healthy &= report(root, "configured Cargo build", &build)?;
    }
    Ok(healthy)
}

fn report(root: &Path, name: &str, path: &Path) -> Result<bool> {
    if let Some(checkout) = foreign_checkout(root, path)? {
        eprintln!(
            "WARNING: {name} {} is inside another checkout's target ({})",
            path.display(),
            checkout.display()
        );
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_targets_are_rejected_even_when_missing_or_nested() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/path-tests")
            .join(format!(
                "{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let own = parent.join("own");
        let other = parent.join("other");
        fs::create_dir_all(own.join(".git")).unwrap();
        fs::create_dir_all(other.join(".git")).unwrap();
        for path in [
            other.join("target"),
            other.join("target/cache/debug"),
            own.join("../other/target/missing"),
        ] {
            assert!(
                foreign_checkout(&own, &path).unwrap().is_some(),
                "{}",
                path.display()
            );
            assert!(!path.exists());
        }
        assert!(
            foreign_checkout(&own, &own.join("target/cache"))
                .unwrap()
                .is_none()
        );
        assert!(
            foreign_checkout(&own, &own.canonicalize().unwrap().join("target/cache"))
                .unwrap()
                .is_none(),
            "canonical Windows output paths must be accepted"
        );
        assert!(
            foreign_checkout(&own, &parent.join("independent"))
                .unwrap()
                .is_some(),
            "a sibling cache inside the outer checkout's target is still foreign"
        );
        assert!(
            foreign_checkout(
                &own,
                &std::env::temp_dir().join("ara-dev-independent-cache")
            )
            .unwrap()
            .is_none()
        );
    }
}
