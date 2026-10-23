//! Preserve failed merges for inspection; validation always precedes the merge commit.

use std::path::Path;

use anyhow::{Result, bail, ensure};

use crate::{check, git};

pub(crate) fn run(
    root: &Path,
    branch: &str,
    push: bool,
    full: bool,
    known_path: Option<&Path>,
) -> Result<bool> {
    // Reject a malformed exception file before starting a merge.
    crate::known::read(root, known_path)?;
    ensure!(
        git::TreeState::read(root)?.status.is_empty(),
        "merge requires a clean checkout"
    );
    ensure!(
        !branch.starts_with('-'),
        "branch must be a ref, not an option"
    );
    let before = git::text(root, ["rev-parse", "HEAD"])?;
    let theirs = git::text(
        root,
        ["rev-parse", "--verify", &format!("{branch}^{{commit}}")],
    )?;
    let status = git::output(root, ["merge", "--no-ff", "--no-commit", &theirs])?;
    if !status.status.success() {
        let conflicts = git::paths(&git::bytes(
            root,
            ["diff", "--name-only", "--diff-filter=U", "-z"],
        )?)?;
        if conflicts.is_empty() {
            bail!("merge failed: {}", String::from_utf8_lossy(&status.stderr));
        }
        if conflicts.iter().any(|p| p == Path::new("Cargo.lock")) {
            git::bytes(root, ["checkout", "--ours", "--", "Cargo.lock"])?;
            git::bytes(root, ["add", "--", "Cargo.lock"])?;
        }
        let others: Vec<_> = conflicts
            .iter()
            .filter(|p| *p != Path::new("Cargo.lock"))
            .collect();
        if !others.is_empty() {
            bail!(
                "merge stopped with conflicts: {}. Resolve them, run cargo check --workspace --all-targets, stage the resolutions and Cargo.lock, then ara-dev check --base {before} before committing; or git merge --abort",
                others
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        // Cargo's normal resolver retains usable lock entries and adds only what merged manifests need.
    }
    let merge_head = git::output(root, ["rev-parse", "--verify", "MERGE_HEAD"])?;
    if !merge_head.status.success() {
        eprintln!("Already up to date; nothing to merge.");
        return Ok(true);
    }
    let mut logs = check::Logs::new(root)?;
    if !logs.command(
        root,
        "workspace-check",
        &["check", "--workspace", "--all-targets"].map(String::from),
    )? {
        bail!("workspace check failed; merge remains uncommitted. Fix it or git merge --abort");
    }
    git::bytes(root, ["add", "--", "Cargo.lock"])?;
    if !check::run(root, &before, false, full, known_path)? {
        bail!(
            "merge gate failed; merge remains uncommitted. Fix it and run ara-dev check --base {before}, or git merge --abort"
        );
    }
    git::bytes(root, ["commit", "-m", &format!("Merge {branch}")])?;
    if push {
        git::bytes(root, ["push"])?;
    }
    println!(
        "Merged {branch}: {}",
        git::text(root, ["rev-parse", "HEAD"])?
    );
    Ok(true)
}
