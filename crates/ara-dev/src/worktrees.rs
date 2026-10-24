//! Only merged, stale, idle, clean targets are candidates; inspection is the default.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, ensure};

use crate::git;

#[derive(Default)]
struct Worktree {
    path: PathBuf,
    branch: Option<String>,
}

fn parse(bytes: &[u8]) -> Result<Vec<Worktree>> {
    let mut trees = Vec::new();
    let mut tree = Worktree::default();
    for field in bytes.split(|b| *b == 0) {
        let field = std::str::from_utf8(field).context("non-UTF-8 worktree record")?;
        if let Some(path) = field.strip_prefix("worktree ") {
            tree.path = path.into();
        } else if let Some(branch) = field.strip_prefix("branch ") {
            tree.branch = Some(branch.into());
        } else if field.is_empty() && !tree.path.as_os_str().is_empty() {
            trees.push(std::mem::take(&mut tree));
        }
    }
    Ok(trees)
}

#[derive(Default)]
struct Safety {
    current: bool,
    merged: bool,
    stale: bool,
    clean: bool,
    safe_path: bool,
    idle: bool,
}

impl Safety {
    fn reasons(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if self.current {
            reasons.push("current worktree");
        }
        if !self.merged {
            reasons.push("branch not merged (or detached)");
        }
        if !self.stale {
            reasons.push("recent writes");
        }
        if !self.clean {
            reasons.push("uncommitted changes");
        }
        if !self.safe_path {
            reasons.push("unsafe path/reparse point");
        }
        if !self.idle {
            reasons.push("Cargo lock busy");
        }
        reasons
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_symlink()
    }
}

struct Target {
    bytes: u64,
    newest: SystemTime,
    locks: Vec<File>,
    safe: bool,
    idle: bool,
}

/// Reject symlinks/junctions anywhere; hold all existing Cargo profile locks.
fn inspect(path: &Path, target: &mut Target) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if is_link(&metadata) {
        target.safe = false;
        return Ok(());
    }
    target.newest = target.newest.max(metadata.modified()?);
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            inspect(&entry?.path(), target)?;
        }
    } else {
        target.bytes += metadata.len();
        if path.file_name().is_some_and(|n| n == ".cargo-lock") {
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            match file.try_lock() {
                Ok(()) => target.locks.push(file),
                Err(std::fs::TryLockError::WouldBlock) => target.idle = false,
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

pub(crate) fn run(root: &Path, yes: bool, hours: u64, base: &str) -> Result<bool> {
    let current = root.canonicalize()?;
    let base = git::text(
        root,
        ["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    let age = Duration::from_secs(hours.checked_mul(3600).context("--hours too large")?);
    let trees = parse(&git::bytes(
        root,
        ["worktree", "list", "--porcelain", "-z"],
    )?)?;
    println!(
        "MODE: {}; minimum age {hours} hours",
        if yes {
            "delete eligible targets"
        } else {
            "dry-run (use --yes to delete)"
        }
    );
    for tree in trees {
        let target_path = tree.path.join("target");
        if !target_path.try_exists()? {
            println!("{:>8}  {}  no target", "0 GiB", tree.path.display());
            continue;
        }
        let mut target = Target {
            bytes: 0,
            newest: SystemTime::UNIX_EPOCH,
            locks: vec![],
            safe: true,
            idle: true,
        };
        if let Err(error) = inspect(&target_path, &mut target) {
            eprintln!(
                "KEEP {}: inspection failed: {error:#}",
                target_path.display()
            );
            continue;
        }
        let canonical_tree = tree.path.canonicalize()?;
        let canonical_target = target_path.canonicalize()?;
        let merged = if let Some(branch) = &tree.branch {
            git::output(root, ["merge-base", "--is-ancestor", branch, &base])?
                .status
                .success()
        } else {
            false
        };
        let state = Safety {
            current: canonical_tree == current,
            merged,
            stale: SystemTime::now()
                .duration_since(target.newest)
                .is_ok_and(|d| d >= age),
            clean: git::TreeState::read(&tree.path)?.status.is_empty(),
            safe_path: target.safe && canonical_target == canonical_tree.join("target"),
            idle: target.idle,
        };
        let reasons = state.reasons();
        println!(
            "{:>8.2} GiB  {}  {}",
            target.bytes as f64 / 1_073_741_824.0,
            tree.path.display(),
            if reasons.is_empty() {
                "eligible".into()
            } else {
                reasons.join(", ")
            }
        );
        if yes && reasons.is_empty() {
            // Recheck identity, branch ancestry, freshness and dirty state at the deletion boundary.
            ensure!(
                target_path.canonicalize()? == canonical_tree.join("target"),
                "target moved during inspection"
            );
            ensure!(
                tree.path.canonicalize()? != current,
                "refusing current worktree"
            );
            let mut recheck = Target {
                bytes: 0,
                newest: SystemTime::UNIX_EPOCH,
                locks: vec![],
                safe: true,
                idle: true,
            };
            // Locks are already held; the second walk checks timestamps/links without reopening them.
            newest(&target_path, &mut recheck)?;
            ensure!(
                recheck.safe
                    && SystemTime::now()
                        .duration_since(recheck.newest)
                        .is_ok_and(|d| d >= age),
                "target changed during inspection"
            );
            ensure!(
                git::TreeState::read(&tree.path)?.status.is_empty(),
                "worktree changed during inspection"
            );
            let branch = git::text(&tree.path, ["symbolic-ref", "--quiet", "HEAD"])?;
            ensure!(
                tree.branch.as_deref() == Some(branch.as_str())
                    && git::output(root, ["merge-base", "--is-ancestor", &branch, &base])?
                        .status
                        .success(),
                "branch changed during inspection"
            );
            // Absolute canonical target is verified within exactly this listed worktree.
            fs::remove_dir_all(&canonical_target)
                .with_context(|| format!("delete {}", canonical_target.display()))?;
            println!("DELETED {}", canonical_target.display());
        }
    }
    Ok(true)
}

fn newest(path: &Path, target: &mut Target) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if is_link(&metadata) {
        target.safe = false;
        return Ok(());
    }
    target.newest = target.newest.max(metadata.modified()?);
    if metadata.is_dir() {
        for e in fs::read_dir(path)? {
            newest(&e?.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_requires_every_safety_condition() {
        for bits in 0..64 {
            let state = Safety {
                current: bits & 1 != 0,
                merged: bits & 2 != 0,
                stale: bits & 4 != 0,
                clean: bits & 8 != 0,
                safe_path: bits & 16 != 0,
                idle: bits & 32 != 0,
            };
            assert_eq!(state.reasons().is_empty(), bits == 62, "{bits}");
        }
    }

    #[test]
    fn nul_worktree_records_preserve_spaces_and_detached_heads() {
        let trees = parse(b"worktree C:/with space\0HEAD abc\0branch refs/heads/main\0\0worktree D:/detached\0HEAD def\0detached\0\0").unwrap();
        assert_eq!(trees.len(), 2);
        assert_eq!(trees[0].path, PathBuf::from("C:/with space"));
        assert_eq!(trees[0].branch.as_deref(), Some("refs/heads/main"));
        assert!(trees[1].branch.is_none());
    }
}
