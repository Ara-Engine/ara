//! Public asset commands preserve one transaction across separate CLI processes.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

fn scratch() -> PathBuf {
    let root = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("asset-cli-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
    fs::create_dir_all(&root).expect("scratch");
    root
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ara"))
        .arg("--root")
        .arg(root)
        .args(["--json", "--author", "human"])
        .args(args)
        .env("ARA_CONFIG_DIR", root.join(".ara/test-profile"))
        .env("ARA_CRASH_DIR", root.join(".ara/test-crashes"))
        .output()
        .expect("CLI process")
}

fn ok(root: &Path, args: &[&str]) -> Value {
    let output = run(root, args);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON result")
}

fn apply(root: &Path, commands: Value) -> Value {
    ok(
        root,
        &[
            "do",
            "--args",
            &json!({"commands":commands,"why":"Exercise public asset operations"}).to_string(),
        ],
    )
}

fn journey() {
    let scratch = scratch();
    let root = scratch.join("project");
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    fs::create_dir_all(&authored).expect("authored root");
    let marker = json!({"name":"Asset proof","main":"main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION});
    fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(&marker),
    )
    .expect("marker");
    fs::write(
        authored.join("main.scene"),
        ara_json::to_canonical_string(
            &json!({"$schema":"ara://schema/scene/1","id":"sc_assets","records":{}}),
        ),
    )
    .expect("scene");
    let external = scratch.join("external OS drop.txt");
    fs::write(&external, b"original external bytes").expect("external input");
    apply(
        &root,
        json!([{"cmd":"folder.create","args":{"path":"drop"}}]),
    );
    let plan = json!({"sources":[external],"destination":"drop","dryRun":true});
    let result = ok(
        &root,
        &[
            "do",
            "asset.import",
            "--args",
            &plan.to_string(),
            "--why",
            "Inspect drop",
        ],
    );
    assert!(result["txn"].is_null());
    assert_eq!(result["outputs"][0]["conflicts"], json!([]));
    assert!(!authored.join("drop/external OS drop.txt").exists());
    let mut import = plan.clone();
    import["dryRun"] = json!(false);
    ok(
        &root,
        &[
            "do",
            "asset.import",
            "--args",
            &import.to_string(),
            "--why",
            "Import drop",
        ],
    );
    let original = authored.join("drop/external OS drop.txt");
    assert_eq!(
        fs::read(&original).expect("imported"),
        b"original external bytes"
    );
    let result = ok(
        &root,
        &[
            "do",
            "asset.import",
            "--args",
            &plan.to_string(),
            "--why",
            "Inspect actual conflict",
        ],
    );
    let conflict = &result["outputs"][0]["conflicts"][0];
    assert_eq!(
        conflict["destinationLogicalPath"],
        "drop/external OS drop.txt"
    );
    assert_eq!(conflict["kind"], "file");
    assert!(conflict.get("existingAssetId").is_some());
    assert!(conflict.get("destination_logical_path").is_none());

    let result = apply(
        &root,
        json!([
            {"cmd":"folder.create","args":{"path":"organized"}},
            {"cmd":"asset.move","args":{"source":"drop/external OS drop.txt","destination":"organized/note.txt"}},
            {"cmd":"asset.rename","args":{"source":"organized/note.txt","name":"renamed.txt"}},
            {"cmd":"asset.duplicate","args":{"source":"organized/renamed.txt","destination":"organized/copy.txt"}},
            {"cmd":"asset.delete","args":{"source":"organized/renamed.txt"}}
        ]),
    );
    assert!(result["txn"].is_string());
    assert_eq!(result["outputs"].as_array().expect("five outputs").len(), 5);
    assert!(!original.exists());
    assert_eq!(
        fs::read(authored.join("organized/copy.txt")).expect("copy"),
        b"original external bytes"
    );
    ok(&root, &["undo"]);
    assert!(original.is_file());
    assert!(!authored.join("organized").exists());
    ok(&root, &["redo"]);
    assert!(!original.exists());
    assert_eq!(
        fs::read(authored.join("organized/copy.txt")).expect("redone"),
        b"original external bytes"
    );

    let invalid = json!({"commands":[
        {"cmd":"folder.create","args":{"path":"must-not-exist"}},
        {"cmd":"asset.move","args":{"source":"missing.txt","destination":"organized/missing.txt"}}
    ],"why":"Prove atomic failure"});
    let failed = run(&root, &["do", "--args", &invalid.to_string()]);
    assert!(!failed.status.success());
    assert!(!authored.join("must-not-exist").exists());
    let result = apply(
        &root,
        json!([{"cmd":"build_profile.create","args":{"name":"Desktop","preset":{"platform":"windows"}}}]),
    );
    assert_eq!(
        result["outputs"][0]["path"],
        "settings/builds/desktop.export.json"
    );
    assert!(root.join("settings/builds/desktop.export.json").is_file());
    ok(&root, &["undo"]);
    assert!(!root.join("settings/builds/desktop.export.json").exists());
}

#[test]
fn legacy_project_asset_journey_requires_explicit_migration() {
    let root = scratch();
    fs::write(root.join("ara.project.json"), "{}").expect("legacy marker");
    let output = run(&root, &["check"]);
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("ARA1914"), "{text}");
    assert!(!root.join("content").exists());
}

#[test]
fn versioned_project_asset_journey() {
    journey();
}

#[test]
fn native_ui_move_round_trips_identity_and_original_bytes() {
    let root = scratch();
    fs::create_dir_all(root.join("content/ui")).expect("UI directory");
    fs::write(root.join("ara.project.json"), r#"{"layoutVersion":4}"#).expect("marker");
    let original = "<ui id=\"hud\"><text id=\"title\" text=\"Ready\" /></ui>\n";
    fs::write(root.join("content/ui/hud.ui"), original).expect("legacy UI");
    let identity = ara_host::Project::open(&root)
        .expect("before")
        .index
        .id_of("ui/hud.ui")
        .expect("UI identity")
        .to_owned();
    apply(
        &root,
        json!([{"cmd":"asset.move","args":{"source":"ui/hud.ui","destination":"ui/moved.ui"}}]),
    );
    assert_eq!(
        ara_host::Project::open(&root)
            .expect("moved")
            .index
            .id_of("ui/moved.ui"),
        Some(identity.as_str())
    );
    ok(&root, &["undo"]);
    assert_eq!(
        fs::read_to_string(root.join("content/ui/hud.ui")).expect("restored bytes"),
        original
    );
    assert!(!root.join("content/ui/moved.ui").exists());
    ok(&root, &["redo"]);
    assert_eq!(
        ara_host::Project::open(&root)
            .expect("redone")
            .index
            .id_of("ui/moved.ui"),
        Some(identity.as_str())
    );
}
