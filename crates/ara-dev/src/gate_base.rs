//! Checkout-local green receipts bound automatic selection to validated history.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::git;

const FORMAT: &str = "ara.last-green/1";

#[derive(Deserialize, Serialize)]
struct Receipt {
    format: String,
    checkout: PathBuf,
    head: String,
    base: String,
    full: bool,
    completed_unix_ms: u128,
    report: PathBuf,
}

pub(crate) struct Base {
    pub(crate) commit: String,
    pub(crate) head: String,
    pub(crate) reason: String,
    pub(crate) bootstrap: bool,
}

fn path(root: &Path) -> PathBuf {
    // This is never the shared/external Cargo target: another checkout's gate
    // must not advance our validated commit or hide our pending batch.
    root.join("target/ara-dev/last-green.json")
}

fn ancestor(root: &Path, older: &str, newer: &str) -> Result<bool> {
    Ok(
        git::output(root, ["merge-base", "--is-ancestor", older, newer])?
            .status
            .success(),
    )
}

fn read(root: &Path, head: &str) -> Result<Receipt> {
    let receipt: Receipt = serde_json::from_slice(&fs::read(path(root))?)?;
    ensure!(receipt.format == FORMAT, "unknown receipt format");
    ensure!(
        receipt.checkout == root.canonicalize()?,
        "receipt belongs to another checkout"
    );
    ensure!(
        matches!(receipt.head.len(), 40 | 64)
            && receipt.head.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid receipt commit"
    );
    ensure!(
        ancestor(root, &receipt.head, head)?,
        "last green is not an ancestor of HEAD"
    );
    Ok(receipt)
}

pub(crate) fn resolve(root: &Path, requested: Option<&str>) -> Result<Base> {
    let head = git::text(root, ["rev-parse", "HEAD"])?;
    let branch = git::output(root, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let on_main = branch.status.success() && branch.stdout == b"main\n";
    let requested = requested.unwrap_or(if on_main { "auto" } else { "main" });
    if requested != "auto" {
        let commit = git::text(root, ["merge-base", "HEAD", requested])
            .context("resolve --base (use an existing local ref)")?;
        return Ok(Base {
            commit,
            head,
            reason: format!("merge-base with {requested}"),
            bootstrap: false,
        });
    }
    match read(root, &head) {
        Ok(receipt) => Ok(Base {
            commit: receipt.head,
            head,
            reason: "last fully green check in this checkout".into(),
            bootstrap: false,
        }),
        Err(error) => Ok(Base {
            commit: head.clone(),
            head,
            reason: format!("auto has no usable green receipt ({error}); full bootstrap required"),
            bootstrap: true,
        }),
    }
}

/// Called only after a passing gate without known exceptions and with a clean start.
/// Recheck HEAD/tree here: a concurrent checkout or edit cannot create a false receipt.
pub(crate) fn record(
    root: &Path,
    head: &str,
    base: &str,
    full: bool,
    report: &Path,
) -> Result<bool> {
    fs::create_dir_all(path(root).parent().context("receipt directory")?)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("target/ara-dev/last-green.lock"))?;
    lock.lock()?;
    if git::text(root, ["rev-parse", "HEAD"])? != head
        || !git::TreeState::read(root)?.status.is_empty()
        || git::output(root, ["rev-parse", "--verify", "MERGE_HEAD"])?
            .status
            .success()
    {
        return Ok(false);
    }
    let receipt = Receipt {
        format: FORMAT.into(),
        checkout: root.canonicalize()?,
        head: head.into(),
        base: base.into(),
        full,
        completed_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        report: report.to_owned(),
    };
    let temporary = path(root).with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec_pretty(&receipt)?)?;
    fs::rename(temporary, path(root)).context("publish last-green receipt")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/green-receipts")
            .join(format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "gate@example.invalid"],
            vec!["config", "user.name", "Gate test"],
        ] {
            git::bytes(&root, args).unwrap();
        }
        fs::write(root.join(".gitignore"), "/target/\n").unwrap();
        commit(&root, "first");
        root
    }

    fn commit(root: &Path, value: &str) {
        fs::write(root.join("source"), value).unwrap();
        git::bytes(root, ["add", "."]).unwrap();
        git::bytes(root, ["commit", "-m", value]).unwrap();
    }

    #[test]
    fn main_bootstraps_then_selects_only_since_the_last_green() {
        let root = fixture();
        let initial = resolve(&root, None).unwrap();
        assert!(
            initial.bootstrap,
            "a first gate on main must never diff HEAD against itself and skip builds"
        );
        assert!(
            record(
                &root,
                &initial.head,
                &initial.commit,
                true,
                Path::new("report.json")
            )
            .unwrap()
        );
        commit(&root, "next batch");
        let next = resolve(&root, None).unwrap();
        assert!(!next.bootstrap);
        assert_eq!(next.commit, initial.head);
        assert_ne!(next.commit, next.head);
        assert_eq!(
            git::changed(&root, &next.commit).unwrap(),
            [PathBuf::from("source")]
        );
        assert!(
            record(
                &root,
                &next.head,
                &next.commit,
                false,
                Path::new("report.json")
            )
            .unwrap()
        );
        assert_eq!(resolve(&root, Some("auto")).unwrap().commit, next.head);
        assert_eq!(
            resolve(&root, Some(&initial.head)).unwrap().commit,
            initial.head
        );
    }

    #[test]
    fn plans_dirty_gates_and_uncommitted_merges_cannot_advance_green_history() {
        let root = fixture();
        let initial = resolve(&root, None).unwrap();
        assert!(
            !path(&root).exists(),
            "selection/plan cannot record success"
        );
        fs::write(root.join("source"), "uncommitted").unwrap();
        assert!(
            !record(
                &root,
                &initial.head,
                &initial.commit,
                false,
                Path::new("report.json")
            )
            .unwrap()
        );
        commit(&root, "different HEAD");
        assert!(
            !record(
                &root,
                &initial.head,
                &initial.commit,
                false,
                Path::new("report.json")
            )
            .unwrap()
        );
        let current = resolve(&root, None).unwrap();
        fs::write(root.join(".git/MERGE_HEAD"), format!("{}\n", initial.head)).unwrap();
        assert!(
            !record(
                &root,
                &current.head,
                &current.commit,
                true,
                Path::new("report.json")
            )
            .unwrap()
        );
        assert!(!path(&root).exists());
    }

    #[test]
    fn rewritten_corrupt_or_foreign_receipts_fall_back_to_full_and_branches_keep_main() {
        let root = fixture();
        let initial = resolve(&root, None).unwrap();
        git::bytes(&root, ["switch", "-c", "agent"]).unwrap();
        commit(&root, "agent change");
        let agent = resolve(&root, None).unwrap();
        assert!(!agent.bootstrap);
        assert_eq!(agent.commit, initial.head);
        assert!(
            record(
                &root,
                &agent.head,
                &agent.commit,
                false,
                Path::new("report.json")
            )
            .unwrap()
        );
        git::bytes(&root, ["switch", "main"]).unwrap();
        assert!(
            resolve(&root, Some("auto")).unwrap().bootstrap,
            "diverged receipt cannot skip changes"
        );
        let mut receipt: Receipt = serde_json::from_slice(&fs::read(path(&root)).unwrap()).unwrap();
        receipt.head = initial.head;
        receipt.checkout = PathBuf::from("other checkout");
        fs::write(path(&root), serde_json::to_vec(&receipt).unwrap()).unwrap();
        assert!(resolve(&root, None).unwrap().bootstrap);
        fs::write(path(&root), "torn JSON").unwrap();
        assert!(resolve(&root, None).unwrap().bootstrap);
    }
}
