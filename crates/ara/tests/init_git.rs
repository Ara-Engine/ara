//! Git merge-driver setup stays scoped to the project repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DRIVER: &str = "ara merge %O %A %B";

fn scratch(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "ara-init-git-{}-{nonce}-{name}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("scratch directory outside the checkout");
    dir
}

fn run(cwd: &Path, program: &str, args: &[&str]) -> Output {
    let mut command = Command::new(program);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(key);
    }
    let output = command
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cwd.join("absent-global-config"))
        .args(args)
        .output()
        .expect("command runs");
    assert!(output.status.success(), "{args:?}: {output:?}");
    output
}

fn ara(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8(run(cwd, env!("CARGO_BIN_EXE_ara"), args).stdout).expect("UTF-8 output")
}

fn git(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8(run(cwd, "git", args).stdout).expect("UTF-8 output")
}

#[test]
fn init_nested_in_repo_leaves_parent_config_untouched() {
    let repo = scratch("nested");
    git(&repo, &["init"]);
    git(
        &repo,
        &["config", "--local", "merge.ara.driver", "existing driver"],
    );
    git(
        &repo,
        &["config", "--local", "merge.ara.name", "Existing name"],
    );
    let before = fs::read(repo.join(".git/config")).expect("repository config");

    let output = ara(&repo, &["init", "target/cursor-scroll-before/fixture"]);

    assert_eq!(
        fs::read(repo.join(".git/config")).expect("repository config after init"),
        before
    );
    assert!(output.contains("git config merge.ara.driver"), "{output}");
    let attributes =
        fs::read_to_string(repo.join("target/cursor-scroll-before/fixture/.gitattributes"))
            .expect("project attributes");
    assert!(attributes.contains("*.scene merge=ara"));
    assert!(attributes.contains("*.prefab merge=ara"));
}

#[test]
fn init_at_repo_root_configures_root_agnostic_driver() {
    let repo = scratch("top level with spaces");
    git(&repo, &["init"]);

    let output = ara(&repo, &["init", ".", "--json"]);

    assert_eq!(
        git(&repo, &["config", "--local", "merge.ara.driver"]).trim(),
        DRIVER
    );
    assert_eq!(
        git(&repo, &["config", "--local", "merge.ara.name"]).trim(),
        "Ara semantic merge"
    );
    let report: serde_json::Value = serde_json::from_str(&output).expect("init report");
    assert_eq!(
        report["git"],
        format!("configured git merge driver `{DRIVER}`")
    );

    // Git supplies temporary paths from the work-tree root, without --root.
    for name in [".merge_base", ".merge_ours", ".merge_theirs"] {
        fs::copy(
            repo.join(ara_host::project_layout::AUTHORING_ROOT)
                .join("scenes/main.scene"),
            repo.join(name),
        )
        .expect("merge input");
    }
    ara(
        &repo,
        &["merge", ".merge_base", ".merge_ours", ".merge_theirs"],
    );
}

#[test]
fn init_outside_repo_prints_configuration_hint() {
    let dir = scratch("outside");
    let output = ara(&dir, &["init"]);
    assert!(output.contains("not a git work tree"), "{output}");
    assert!(
        output.contains(&format!("git config merge.ara.driver \"{DRIVER}\"")),
        "{output}"
    );
    assert!(!dir.join(".git").exists());
}

#[test]
fn init_nested_configures_parent_driver_only_when_requested() {
    let repo = scratch("explicit");
    git(&repo, &["init"]);
    ara(&repo, &["init", "game", "--configure-git"]);
    assert_eq!(
        git(&repo, &["config", "--local", "merge.ara.driver"]).trim(),
        DRIVER
    );
}

#[test]
fn merge_discovers_project_from_input_paths_outside_current_directory() {
    let dir = scratch("file-discovery");
    ara(&dir, &["init", "game"]);
    let scene = dir
        .join("game")
        .join(ara_host::project_layout::AUTHORING_ROOT)
        .join("scenes/main.scene");
    let scene = scene.to_str().expect("UTF-8 scene path");
    ara(&dir, &["merge", scene, scene, scene]);
}
