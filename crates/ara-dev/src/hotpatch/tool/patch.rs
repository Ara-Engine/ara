//! Upgrade only the recorded tool patch; keep unknown edits and previous bytes.
use anyhow::{Context, Result, ensure};
use std::{fs, path::Path};

pub(super) fn prepare(source: &Path, cache: &Path) -> Result<()> {
    prepare_with(source, cache, include_bytes!("../dx-native-watch.patch"))
}

fn diff(source: &Path) -> Result<Vec<u8>> {
    let output = super::command("git")
        .arg("-C")
        .arg(source)
        .args([
            "diff",
            "--no-color",
            "--binary",
            "--abbrev=7",
            "--unified=0",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ])
        .output()?;
    ensure!(
        output.status.success(),
        "ARA7600: cannot inspect optional dx source"
    );
    Ok(output.stdout)
}

fn apply(source: &Path, patch: &Path, reverse: bool) -> Result<()> {
    let mut command = super::command("git");
    command
        .arg("-C")
        .arg(source)
        .args(["apply", "--unidiff-zero"]);
    if reverse {
        command.arg("--reverse");
    }
    command.arg(patch);
    super::run(&mut command)
}

fn prepare_with(source: &Path, cache: &Path, current: &[u8]) -> Result<()> {
    let before = diff(source)?;
    if before.trim_ascii() == current.trim_ascii() {
        fs::write(cache, current)?;
        return Ok(());
    }
    if !before.trim_ascii().is_empty() {
        let previous = fs::read(cache)
            .context("ARA7600: unknown cached dx edits; preserve them before reinstalling")?;
        ensure!(
            before.trim_ascii() == previous.trim_ascii(),
            "ARA7600: cached dx has unrecognized edits; preserve them before reinstalling"
        );
        let backup = cache.with_extension(format!(
            "previous-{}.patch",
            blake3::hash(&previous).to_hex()
        ));
        fs::write(&backup, &previous)?;
        apply(source, &backup, true)?;
    }
    fs::write(cache, current)?;
    apply(source, cache, false)?;
    ensure!(
        diff(source)?.trim_ascii() == current.trim_ascii(),
        "ARA7600: optional dx source does not match its pinned patch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let scratch = tempfile::tempdir().unwrap();
        let source = scratch.path().join("source");
        fs::create_dir(&source).unwrap();
        let output = super::super::command("git")
            .arg("-C")
            .arg(&source)
            .arg("init")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            super::super::command("git")
                .arg("-C")
                .arg(&source)
                .args(["config", "--local", "core.autocrlf", "false"])
                .status()
                .unwrap()
                .success()
        );
        fs::write(source.join("note"), "baseline\n").unwrap();
        assert!(
            super::super::command("git")
                .arg("-C")
                .arg(&source)
                .args(["add", "note"])
                .status()
                .unwrap()
                .success()
        );
        // The index is sufficient for diff/apply; no identity or profile config is used.
        (scratch, source)
    }

    #[test]
    fn upgrades_recorded_patch_and_keeps_its_backup() {
        let (scratch, source) = fixture();
        let cache = scratch.path().join("native.patch");
        fs::write(source.join("note"), "old patch\n").unwrap();
        let old = diff(&source).unwrap();
        fs::write(&cache, &old).unwrap();
        fs::write(source.join("note"), "new patch\n").unwrap();
        let current = diff(&source).unwrap();
        fs::write(source.join("note"), "old patch\n").unwrap();
        prepare_with(&source, &cache, &current).unwrap();
        assert_eq!(fs::read(source.join("note")).unwrap(), b"new patch\n");
        assert_eq!(fs::read(&cache).unwrap(), current);
        let backup =
            cache.with_extension(format!("previous-{}.patch", blake3::hash(&old).to_hex()));
        assert_eq!(fs::read(backup).unwrap(), old);
        prepare_with(&source, &cache, &current).unwrap();
    }

    #[test]
    fn unknown_edits_are_preserved() {
        let (scratch, source) = fixture();
        let cache = scratch.path().join("native.patch");
        fs::write(source.join("note"), "recorded patch\n").unwrap();
        let old = diff(&source).unwrap();
        fs::write(&cache, &old).unwrap();
        fs::write(source.join("note"), "user edit\n").unwrap();
        assert!(prepare_with(&source, &cache, b"different patch").is_err());
        assert_eq!(fs::read(source.join("note")).unwrap(), b"user edit\n");
        assert_eq!(fs::read(cache).unwrap(), old);
    }
}
