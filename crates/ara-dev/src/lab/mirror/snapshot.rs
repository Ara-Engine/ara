//! Private Git ref snapshots. The caller's index, branch and working files are untouched.
use anyhow::{Context, Result, ensure};
use std::{
    path::Path,
    process::{Command, Stdio},
};

pub(super) fn create(root: &Path, base: &Path, reference: &str) -> Result<String> {
    let folder = base.join("mirror/git");
    std::fs::create_dir_all(&folder)?;
    let index = folder.join("snapshot.index");
    let git = |args: &[&str], input: Option<&str>| -> Result<String> {
        let mut command = Command::new("git");
        command
            .current_dir(root)
            .env("GIT_INDEX_FILE", &index)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        super::super::build::background(&mut command);
        let mut child = command.spawn()?;
        if let Some(input) = input {
            use std::io::Write;
            child
                .stdin
                .take()
                .context("snapshot Git input")?
                .write_all(input.as_bytes())?;
        }
        let result = child.wait_with_output()?;
        ensure!(
            result.status.success(),
            "snapshot git: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(String::from_utf8(result.stdout)?.trim().to_owned())
    };
    git(&["read-tree", "HEAD"], None)?;
    let mut args = vec!["add", "-A", "--"];
    for item in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        ".cargo",
        "crates",
        "shaders",
        "samples",
        "benchmarks",
    ] {
        if root.join(item).exists() {
            args.push(item);
        }
    }
    git(&args, None)?;
    let tree = git(&["write-tree"], None)?;
    let parent = git(&["rev-parse", "HEAD"], None)?;
    let previous = git(&["rev-parse", reference], None).ok();
    if let Some(previous) = previous
        && git(&["rev-parse", &format!("{previous}^{{tree}}")], None)? == tree
    {
        return Ok(previous);
    }
    let commit = git(
        &["commit-tree", &tree, "-p", &parent],
        Some("Fidelity Lab live workspace snapshot\n"),
    )?;
    git(&["update-ref", reference, &commit], None)?;
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_keeps_the_agents_index_branch_and_files_intact() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/lab-tests")
            .join(super::super::super::session::id());
        std::fs::create_dir_all(root.join("crates/fixture"))?;
        let git = |args: &[&str]| -> Result<String> {
            let output = Command::new("git").current_dir(&root).args(args).output()?;
            ensure!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(String::from_utf8(output.stdout)?)
        };
        git(&["init", "--quiet"])?;
        git(&["config", "user.name", "Ara Lab Test"])?;
        git(&["config", "user.email", "lab-test@example.invalid"])?;
        std::fs::write(root.join(".gitignore"), "target/\n")?;
        std::fs::write(root.join("crates/fixture/lib.rs"), "baseline\n")?;
        git(&["add", "."])?;
        git(&["commit", "--quiet", "-m", "Baseline"])?;
        std::fs::write(root.join("crates/fixture/lib.rs"), "staged work\n")?;
        git(&["add", "crates"])?;
        let index = std::fs::read(root.join(".git/index"))?;
        let head = git(&["rev-parse", "HEAD"])?;
        std::fs::write(root.join("crates/fixture/lib.rs"), "live work\n")?;
        std::fs::write(root.join("crates/fixture/sibling.slang"), "experiment\n")?;
        let base = root.join("target/lab");
        let commit = create(&root, &base, "refs/ara-lab/test")?;
        assert_eq!(
            git(&["show", &format!("{commit}:crates/fixture/lib.rs")])?,
            "live work\n"
        );
        assert_eq!(
            git(&["show", &format!("{commit}:crates/fixture/sibling.slang")])?,
            "experiment\n"
        );
        assert_eq!(std::fs::read(root.join(".git/index"))?, index);
        assert_eq!(git(&["rev-parse", "HEAD"])?, head);
        assert_eq!(
            std::fs::read_to_string(root.join("crates/fixture/lib.rs"))?,
            "live work\n"
        );
        assert_eq!(create(&root, &base, "refs/ara-lab/test")?, commit);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
