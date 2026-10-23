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
    let build = std::path::Path::new(command.get_program())
        .file_stem()
        .is_some_and(|name| name == "cargo" || name == "dotnet");
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(if build { 7200 } else { 180 });
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
pub fn name(version: &str) -> String {
    format!("ara-{version}-windows-x64")
}
