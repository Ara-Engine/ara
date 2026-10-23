//! Git subprocesses preserve NUL-separated paths and never invoke a shell.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail, ensure};

pub(crate) fn output<I, S>(root: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .context("run git")
}

pub(crate) fn bytes<I, S>(root: &Path, args: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let result = output(root, args)?;
    ensure!(
        result.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(result.stdout)
}

pub(crate) fn text<I, S>(root: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Ok(String::from_utf8(bytes(root, args)?)
        .context("git returned non-UTF-8 text")?
        .trim()
        .into())
}

pub(crate) fn paths(bytes: &[u8]) -> Result<Vec<PathBuf>> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            Ok(PathBuf::from(
                std::str::from_utf8(p).context("non-UTF-8 git path")?,
            ))
        })
        .collect()
}

pub(crate) fn files(root: &Path) -> Result<Vec<PathBuf>> {
    paths(&bytes(
        root,
        [
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?)
}

/// Include unstaged, staged and new files as well as branch commits.
pub(crate) fn changed(root: &Path, base: &str) -> Result<Vec<PathBuf>> {
    let mut files = paths(&bytes(
        root,
        ["diff", "--name-only", "--no-renames", "-z", base, "--"],
    )?)?;
    files.extend(paths(&bytes(
        root,
        ["ls-files", "-z", "--others", "--exclude-standard"],
    )?)?);
    files.sort();
    files.dedup();
    Ok(files)
}

/// Compare content as well as status: overwriting an already-dirty file must fail.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TreeState {
    pub(crate) status: Vec<u8>,
    staged: Vec<u8>,
    unstaged: Vec<u8>,
    untracked: Vec<(PathBuf, Vec<u8>)>,
}

impl TreeState {
    pub(crate) fn read(root: &Path) -> Result<Self> {
        let status = bytes(
            root,
            ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        let staged = bytes(
            root,
            [
                "diff",
                "--cached",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
            ],
        )?;
        let unstaged = bytes(root, ["diff", "--binary", "--no-ext-diff", "--no-textconv"])?;
        let untracked = paths(&bytes(
            root,
            ["ls-files", "-z", "--others", "--exclude-standard"],
        )?)?
        .into_iter()
        .map(|path| {
            let hash = bytes(
                root,
                [
                    OsStr::new("hash-object"),
                    OsStr::new("--"),
                    path.as_os_str(),
                ],
            )?;
            Ok((path, hash))
        })
        .collect::<Result<_>>()?;
        Ok(Self {
            status,
            staged,
            unstaged,
            untracked,
        })
    }

    pub(crate) fn verify(&self, after: &Self) -> Result<()> {
        if self != after {
            bail!(
                "tests changed the tree (including existing dirty-file contents); inspect git status and git diff"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_tree_comparison_detects_same_status_but_changed_content() {
        let before = TreeState {
            status: b" M a\0".to_vec(),
            staged: vec![],
            unstaged: vec![1],
            untracked: vec![],
        };
        let mut after = TreeState {
            status: b" M a\0".to_vec(),
            staged: vec![],
            unstaged: vec![1],
            untracked: vec![],
        };
        assert!(before.verify(&after).is_ok());
        after.unstaged.push(2);
        assert!(before.verify(&after).is_err());
        after.unstaged = vec![1];
        after.untracked.push(("new file".into(), vec![3]));
        assert!(before.verify(&after).is_err());
    }
}
