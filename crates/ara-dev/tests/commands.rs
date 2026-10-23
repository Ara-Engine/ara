//! Exercise real git/Cargo transactions in disposable repositories under target/.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

static SERIAL: Mutex<()> = Mutex::new(());
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn scratch(label: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-dev/tests")
        .join(format!(
            "{label}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&path).unwrap();
    let path = path.canonicalize().unwrap();
    #[cfg(windows)]
    let path = PathBuf::from(path.to_string_lossy().trim_start_matches(r"\\?\"));
    path
}

fn command(root: &Path, program: &str, args: &[&str]) -> Output {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("CARGO_BUILD_BUILD_DIR", root.join("target"))
        .env("CARGO_BUILD_JOBS", "2")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("empty-gitconfig"))
        .output()
        .unwrap()
}

fn ok(root: &Path, program: &str, args: &[&str]) -> Output {
    let out = command(root, program, args);
    assert!(
        out.status.success(),
        "{program} {args:?}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git(root: &Path, args: &[&str]) -> Output {
    ok(root, "git", args)
}

fn fixture() -> PathBuf {
    let root = scratch("repo");
    fs::write(root.join(".gitignore"), "/target/\n/empty-gitconfig\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"core\", \"tool\", \"diag\"]\n",
    )
    .unwrap();
    for (dir, name, extra) in [
        ("core", "mini-core", ""),
        (
            "tool",
            "mini-tool",
            "\n[dependencies]\nrenamed = { package = \"mini-core\", path = \"../core\", optional = true }\n",
        ),
        ("diag", "ara-diag", ""),
    ] {
        fs::create_dir_all(root.join(dir).join("src")).unwrap();
        fs::write(
            root.join(dir).join("Cargo.toml"),
            format!("[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\n{extra}"),
        )
        .unwrap();
        fs::write(
            root.join(dir).join("src/lib.rs"),
            "pub fn value() -> u32 {\n    1\n}\n",
        )
        .unwrap();
    }
    fs::create_dir_all(root.join("diag/tests")).unwrap();
    fs::write(
        root.join("diag/tests/coverage.rs"),
        "#[test]\nfn catalog() {\n    assert_eq!(ara_diag::value(), 1);\n}\n",
    )
    .unwrap();
    ok(&root, "cargo", &["generate-lockfile", "--offline"]);
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.email", "ara-dev@example.invalid"]);
    git(&root, &["config", "user.name", "ara-dev tests"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fixture"]);
    root
}

#[test]
fn check_selects_reverse_dependencies_and_keeps_json_stdout_clean() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    let before = git(&root, &["status", "--porcelain"]).stdout;
    let output = ok(&root, env!("CARGO_BIN_EXE_ara-dev"), &["check", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(
        report["affected"],
        serde_json::json!(["mini-core", "mini-tool"])
    );
    assert_eq!(before, git(&root, &["status", "--porcelain"]).stdout);
}

#[test]
fn merge_commits_only_after_gate_and_stops_without_committing_other_conflicts() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    git(&root, &["checkout", "-b", "feature"]);
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    git(&root, &["commit", "-am", "feature"]);
    git(&root, &["checkout", "main"]);
    ok(&root, env!("CARGO_BIN_EXE_ara-dev"), &["merge", "feature"]);
    assert_eq!(
        String::from_utf8(git(&root, &["show", "-s", "--format=%P"]).stdout)
            .unwrap()
            .split_whitespace()
            .count(),
        2
    );
    assert!(git(&root, &["status", "--porcelain"]).stdout.is_empty());
    git(&root, &["checkout", "-b", "conflict"]);
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("automatically @generated", "theirs @generated"),
    )
    .unwrap();
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    3\n}\n",
    )
    .unwrap();
    git(&root, &["commit", "-am", "theirs"]);
    git(&root, &["checkout", "main"]);
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("automatically @generated", "ours @generated"),
    )
    .unwrap();
    fs::write(
        root.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    4\n}\n",
    )
    .unwrap();
    git(&root, &["commit", "-am", "ours"]);
    let before = git(&root, &["rev-parse", "HEAD"]).stdout;
    let output = command(&root, env!("CARGO_BIN_EXE_ara-dev"), &["merge", "conflict"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("core/src/lib.rs"));
    assert!(
        !fs::read_to_string(root.join("Cargo.lock"))
            .unwrap()
            .contains("<<<<<<<")
    );
    assert_eq!(
        git(&root, &["diff", "--name-only", "--diff-filter=U"]).stdout,
        b"core/src/lib.rs\n"
    );
    assert_eq!(before, git(&root, &["rev-parse", "HEAD"]).stdout);
    git(&root, &["merge", "--abort"]);
}

#[test]
fn lockfile_only_conflict_is_regenerated_without_upgrading_dependencies() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    git(&root, &["checkout", "-b", "feature"]);
    // Both sides edit the same generated header; their manifests stay compatible.
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("automatically @generated", "feature @generated"),
    )
    .unwrap();
    git(&root, &["commit", "-am", "theirs lock"]);
    git(&root, &["checkout", "main"]);
    fs::write(
        root.join("Cargo.lock"),
        lock.replace("automatically @generated", "main @generated"),
    )
    .unwrap();
    git(&root, &["commit", "-am", "ours lock"]);
    ok(&root, env!("CARGO_BIN_EXE_ara-dev"), &["merge", "feature"]);
    let actual = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    assert!(!actual.contains("<<<<<<<"));
    assert_eq!(actual.matches("version = \"0.1.0\"").count(), 3);
    assert!(git(&root, &["status", "--porcelain"]).stdout.is_empty());
}

#[test]
fn cleanup_keeps_current_dirty_unmerged_and_active_targets() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let root = fixture();
    let other = scratch("worktree");
    git(
        &root,
        &["worktree", "add", "-b", "merged", other.to_str().unwrap()],
    );
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("target/keep"), "current").unwrap();
    fs::create_dir_all(other.join("target/debug")).unwrap();
    fs::write(other.join("target/debug/cache"), "cache").unwrap();
    fs::write(other.join("dirty"), "untracked").unwrap();
    ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["clean-worktrees", "--yes", "--hours", "0"],
    );
    assert!(other.join("target").exists(), "dirty tree was deleted");
    fs::remove_file(other.join("dirty")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(other.join("target/debug/.cargo-lock"))
        .unwrap();
    lock.lock().unwrap();
    ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["clean-worktrees", "--yes", "--hours", "0"],
    );
    assert!(
        other.join("target").exists(),
        "active Cargo target was deleted"
    );
    drop(lock);
    // A clean but unmerged branch must also be kept.
    fs::write(
        other.join("core/src/lib.rs"),
        "pub fn value() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    git(&other, &["commit", "-am", "unmerged"]);
    ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["clean-worktrees", "--yes", "--hours", "0"],
    );
    assert!(other.join("target").exists(), "unmerged target was deleted");
    git(&root, &["merge", "--ff-only", "merged"]);
    ok(&root, env!("CARGO_BIN_EXE_ara-dev"), &["clean-worktrees"]);
    assert!(
        other.join("target").exists(),
        "default must be dry-run and fresh targets kept"
    );
    ok(
        &root,
        env!("CARGO_BIN_EXE_ara-dev"),
        &["clean-worktrees", "--yes", "--hours", "0"],
    );
    assert!(!other.join("target").exists(), "eligible target was kept");
    assert!(
        root.join("target/keep").exists(),
        "current target was deleted"
    );
}
