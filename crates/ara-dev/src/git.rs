//! Git subprocesses preserve NUL-separated paths and never invoke a shell.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

/// Rust opens native/extended Windows paths; Git hashes the inherited file without path translation.
pub(crate) fn hash_file(root: &Path, path: &Path) -> Result<String> {
    let result = Command::new("git")
        .current_dir(root)
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::from(
            std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?,
        ))
        .output()
        .context("hash measured artifact")?;
    ensure!(
        result.status.success(),
        "git hash failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
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

/// One Git process reads the base manifests; absent paths represent new packages.
pub(crate) fn read_at(
    root: &Path,
    base: &str,
    paths: &[PathBuf],
) -> Result<BTreeMap<PathBuf, Option<String>>> {
    let mut queries = Vec::new();
    for path in paths {
        let path = path
            .to_str()
            .context("non-UTF-8 manifest path")?
            .replace('\\', "/");
        ensure!(!path.contains(['\n', '\r']), "newline in manifest path");
        writeln!(queries, "{base}:{path}")?;
    }
    let mut child = Command::new("git")
        .current_dir(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().context("git batch stdin")?;
    // Drain stdout while feeding requests: either pipe can exceed Windows' buffer.
    let writer = std::thread::spawn(move || input.write_all(&queries));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("git request writer panicked"))??;
    ensure!(
        output.status.success(),
        "git batch: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut rest = output.stdout.as_slice();
    let mut result = BTreeMap::new();
    for path in paths {
        let end = rest
            .iter()
            .position(|b| *b == b'\n')
            .context("git batch header")?;
        let header = std::str::from_utf8(&rest[..end])?;
        rest = &rest[end + 1..];
        if header.ends_with(" missing") {
            result.insert(path.clone(), None);
        } else {
            let size: usize = header
                .split_whitespace()
                .last()
                .context("git blob size")?
                .parse()?;
            ensure!(rest.len() > size, "truncated git blob");
            result.insert(
                path.clone(),
                Some(std::str::from_utf8(&rest[..size])?.to_owned()),
            );
            rest = &rest[size + 1..];
        }
    }
    Ok(result)
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
    fn artifact_hashes_support_canonical_windows_paths() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        let directory = target.join("ara-dev/hash-tests");
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join(format!("artifact-{}.bin", std::process::id()));
        std::fs::write(&file, b"native-path artifact\0\xff\n").unwrap();
        let expected = text(
            &root,
            [
                OsStr::new("hash-object"),
                OsStr::new("--no-filters"),
                file.as_os_str(),
            ],
        )
        .unwrap();
        assert_eq!(hash_file(&root, &file).unwrap(), expected);
        assert_eq!(
            hash_file(&root, &file.canonicalize().unwrap()).unwrap(),
            expected
        );
    }

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
