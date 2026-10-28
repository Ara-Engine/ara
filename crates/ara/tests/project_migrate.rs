//! CLI migration never opens the old runtime, and its Undo remains journaled across processes.
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn cli(root: &PathBuf, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ara"));
    command
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("--author")
        .arg("human")
        .args(args);
    for variable in ["CODEX_THREAD_ID", "CLAUDECODE", "ARA_AUTHOR"] {
        command.env_remove(variable);
    }
    let profile = root.with_extension("profile");
    command
        .env("ARA_CONFIG_DIR", profile.join("config"))
        .env("ARA_CRASH_DIR", profile.join("crashes"));
    command.output().expect("CLI executes")
}
#[test]
fn dry_run_migrate_check_undo_and_blocked_reopen_cross_process() {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("ara-cli-project-migration"));
    let root = target.join("project-migrate-cli").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join("game/scenes")).expect("fixture");
    fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"layoutVersion":3,"name":"Old Preview","main":"scenes/main.scene"}),
        ),
    )
    .expect("marker");
    let scene = ara_json::to_canonical_string(
        &json!({"$schema":"ara://schema/scene/1","id":"sc_main","records":{}}),
    );
    fs::write(root.join("game/scenes/main.scene"), &scene).expect("scene");
    let output = cli(&root, &["project", "migrate", "--dry-run"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: Value = serde_json::from_slice(&output.stdout).expect("plan JSON");
    assert_eq!(plan["needsMigration"], true);
    assert_eq!(plan["authoredRoot"], "content");
    assert_eq!(plan["toVersion"], 4);
    assert!(!root.join(".ara").exists());
    let output = cli(&root, &["check"]);
    assert!(!output.status.success(), "legacy runtime cannot open");
    assert!(
        !root.join(".ara").exists(),
        "rejected normal open cannot create state"
    );
    let output = cli(&root, &["project", "migrate"]);
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("content/scenes/main.scene")).expect("migrated scene"),
        scene
    );
    let output = cli(&root, &["check"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let output = cli(&root, &["undo"]);
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join("game/scenes/main.scene").is_file());
    assert!(!root.join("content").exists());
    let output = cli(&root, &["check"]);
    assert!(!output.status.success());
    let diagnostics = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(diagnostics.contains("ARA1914"), "{diagnostics}");
    let journal = fs::read_to_string(root.join(".ara/journal")).expect("journal");
    assert_eq!(
        journal
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry["t"] == "txn")
            .count(),
        2,
        "migration and its single Undo are journaled"
    );
}
