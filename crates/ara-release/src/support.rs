use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Keep compiler output in a named log and propagate the actual exit status.
pub fn run(command: &mut Command, log: &Path) -> Result<()> {
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    eprintln!("Running {command:?}; log: {}", log.display());
    let output = fs::File::create(log)?;
    let mut child = command
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(output))
        .spawn()
        .with_context(|| format!("starting {command:?}"))?;
    // Builds get two hours; an editor opening a sample imports all of its assets.
    let seconds = match std::path::Path::new(command.get_program())
        .file_stem()
        .and_then(|name| name.to_str())
    {
        Some("cargo" | "dotnet") => 7200,
        Some("ara-editor" | "ara-samples") => 900,
        _ => 180,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{command:?} timed out; see {}", log.display());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    ensure!(
        status.success(),
        "{command:?} exited {status}; see {}",
        log.display()
    );
    Ok(())
}
pub fn json(path: &Path, value: &Value) -> Result<()> {
    fs::write(
        path,
        ara_json::to_canonical_string(&ara_json::sort_keys(value)),
    )?;
    Ok(())
}
pub fn files(root: &Path) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            ensure!(
                !kind.is_symlink(),
                "release input contains a symlink: {}",
                entry.path().display()
            );
            if kind.is_dir() {
                walk(&entry.path(), out)?;
            } else if kind.is_file() {
                out.push(entry.path());
            }
        }
        Ok(())
    }
    let mut result = Vec::new();
    walk(root, &mut result)?;
    result.sort();
    Ok(result)
}
pub fn copy(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target.parent().context("copy needs a parent")?)?;
    fs::copy(source, target)
        .with_context(|| format!("copying {} to {}", source.display(), target.display()))?;
    Ok(())
}
pub fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    for file in files(source)? {
        copy(&file, &target.join(file.strip_prefix(source)?))?;
    }
    Ok(())
}
pub fn dist(root: &Path) -> PathBuf {
    root.join("target/release-dist")
}

/// Compiler outputs may live in a warm external target; staged packages stay local.
pub fn profile(root: &Path, profile: &str) -> PathBuf {
    profile_at(
        root,
        profile,
        std::env::var_os("CARGO_TARGET_DIR"),
        std::env::var_os("CARGO_BUILD_TARGET"),
    )
}

fn profile_at(
    root: &Path,
    profile: &str,
    target: Option<std::ffi::OsString>,
    triple: Option<std::ffi::OsString>,
) -> PathBuf {
    let mut path = match target {
        Some(target) => root.join(target),
        None => root.join("target"),
    };
    if let Some(triple) = triple {
        path.push(triple);
    }
    path.join(profile)
}

pub fn name(version: &str) -> String {
    format!("ara-{version}-windows-x64")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn warm_compiler_outputs_do_not_move_staged_packages() {
        let root = std::env::current_dir().expect("test working directory");
        assert_eq!(
            profile_at(&root, "release", None, None),
            root.join("target/release")
        );
        assert_eq!(
            profile_at(
                &root,
                "release",
                Some("warm".into()),
                Some("x86_64-pc-windows-msvc".into())
            ),
            root.join("warm/x86_64-pc-windows-msvc/release")
        );
        let external = std::env::temp_dir().join("ara-release-warm");
        assert_eq!(
            profile_at(&root, "release", Some(external.clone().into()), None),
            external.join("release")
        );
        assert_eq!(dist(&root), root.join("target/release-dist"));
    }
}
