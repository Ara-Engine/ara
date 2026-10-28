use super::*;
use ara_doc::{Op, file_state::record_id};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn scratch(name: &str) -> PathBuf {
    let root = std::env::var_os("ARA_TEST_ROOT")
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ara-migration-history-tests")
        .join(format!(
            "{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
    fs::create_dir_all(&root).expect("scratch");
    root.canonicalize().expect("canonical scratch")
}
fn journal(root: &Path, entries: &[Value]) {
    fs::create_dir_all(root.join(".ara")).expect("state directory");
    fs::write(
        root.join(".ara/journal"),
        entries
            .iter()
            .map(|entry| serde_json::to_string(entry).expect("entry") + "\n")
            .collect::<String>(),
    )
    .expect("journal");
}
fn file(path: &str, state: &FileState, prev: Option<&FileState>) -> Value {
    json!({"op":"file","doc":"pj_project","id":record_id(path),"path":path,"state":state,"prev":prev})
}
fn blobs(root: &Path) -> Vec<String> {
    let mut names = fs::read_dir(root.join(".ara/file-history"))
        .expect("history store")
        .map(|entry| {
            entry
                .expect("blob")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn file_snapshots_migrate_read_only_then_support_undo_with_valid_references_and_ids() {
    let root = scratch("roundtrip");
    for directory in ["levels", "ui", "assets/empty"] {
        fs::create_dir_all(root.join(directory)).expect("folder");
    }
    fs::write(
        root.join("ara.project.json"),
        r#"{"main":"levels/main.scene","ui":{"resources":"ui/custom.json"}}"#,
    )
    .expect("marker");
    let current=br#"{"id":"sc_history","records":{"e_use":{"prefab":"@assets/box.prefab#pf_box","name":"assets/Display"}}}"#;
    let previous=br#"{"id":"sc_history","records":{"e_use":{"prefab":"@assets/old.prefab#pf_old","name":"assets/OldLabel"}}}"#;
    let ui=br#"<ui id="outpost"><image id="icon" src="@assets/icon.png#as_icon"/><text id="label" text="assets/Display"/></ui>"#;
    let resources=br#"{"fonts":["assets/font.ttf"],"images":{"icon":"assets/icon.png"},"click":"assets/click.ogg","label":"assets/Display"}"#;
    let binary = b"@assets/unchanged.bin#as_binary";
    for (path, bytes) in [
        ("levels/main.scene", current.as_slice()),
        ("ui/game.ui", ui.as_slice()),
        ("ui/custom.json", resources.as_slice()),
        ("assets/raw.bin", binary.as_slice()),
    ] {
        fs::write(root.join(path), bytes).expect("authored file");
    }
    fs::write(
        root.join("assets/box.prefab"),
        r#"{"id":"pf_box","records":{}}"#,
    )
    .expect("prefab");
    let history = FileHistory::new(&root);
    let current_state = history.put(current).expect("current snapshot");
    let previous_state = history.put(previous).expect("previous snapshot");
    let ui_state = history.put(ui).expect("UI snapshot");
    let resource_state = history.put(resources).expect("resource snapshot");
    let binary_state = history.put(binary).expect("binary snapshot");
    let directory_state = FileState::directory();
    let ui_id = crate::ui_document::header(std::str::from_utf8(ui).expect("UI"), "ui/game.ui")
        .expect("UI identity")
        .1;
    journal(
        &root,
        &[
            json!({"t":"txn","txn":{"ops":[file("levels/main.scene",&current_state,Some(&previous_state)),file("ui/game.ui",&ui_state,None),file("ui/custom.json",&resource_state,None),file("assets/raw.bin",&binary_state,None),file("assets/empty",&directory_state,None)]}}),
            json!({"t":"fileFlush","path":"levels/main.scene","rev":1,"state":current_state}),
            json!({"t":"fileFlush","path":"ui/game.ui","rev":1,"state":ui_state}),
        ],
    );
    let before = blobs(&root);
    let journal_before = fs::read(root.join(".ara/journal")).expect("journal before");
    let plan = Migration::plan(&root, None).expect("dry run");
    assert_eq!(
        blobs(&root),
        before,
        "planning must not put or rewrite any immutable blob"
    );
    assert_eq!(
        fs::read(root.join(".ara/journal")).expect("unchanged journal"),
        journal_before
    );
    assert!(!root.join(".ara/lock").exists());
    assert_eq!(
        fs::read(root.join("levels/main.scene")).expect("unmoved source"),
        current
    );
    // An already present matching immutable target is reused by a fresh plan.
    let prepared = plan
        .rewrites
        .iter()
        .find(|rewrite| rewrite.target.parent() == Some(root.join(".ara/file-history").as_path()))
        .expect("planned blob")
        .after
        .clone();
    let reused = history.put(&prepared).expect("fixture seeds matching blob");
    let plan = Migration::plan(&root, None).expect("reuse plan");
    assert!(
        !plan
            .rewrites
            .iter()
            .any(|rewrite| rewrite.target == root.join(".ara/file-history").join(&reused.hash))
    );
    plan.apply().expect("migrate");
    let entries = fs::read_to_string(root.join(".ara/journal"))
        .expect("journal")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("entry"))
        .collect::<Vec<_>>();
    let ops = entries[0]["txn"]["ops"].as_array().expect("ops");
    let migrated: FileState =
        serde_json::from_value(ops[0]["state"].clone()).expect("migrated state");
    let migrated_prev: FileState =
        serde_json::from_value(ops[0]["prev"].clone()).expect("migrated previous");
    assert_ne!(migrated.hash, current_state.hash);
    assert_eq!(entries[1]["state"], ops[0]["state"]);
    let authored = root.join(project_layout::AUTHORING_ROOT);
    assert_eq!(
        FileState::bytes(&fs::read(authored.join("levels/main.scene")).expect("current bytes")),
        migrated,
        "flush metadata must match the migrated file"
    );
    let value: Value =
        serde_json::from_slice(&history.read(&migrated).expect("new snapshot")).expect("JSON");
    assert_eq!(value["id"], "sc_history");
    assert_eq!(value["records"]["e_use"]["prefab"], "@box.prefab#pf_box");
    assert_eq!(value["records"]["e_use"]["name"], "assets/Display");
    let migrated_ui: FileState = serde_json::from_value(ops[1]["state"].clone()).expect("UI state");
    let markup = String::from_utf8(history.read(&migrated_ui).expect("UI bytes")).expect("UI UTF8");
    assert!(markup.contains(&format!("<!-- ara:asset {ui_id} -->")));
    assert!(markup.contains("@icon.png#as_icon"));
    assert!(markup.contains("text=\"assets/Display\""));
    assert_eq!(entries[2]["state"], ops[1]["state"]);
    assert_eq!(
        FileState::bytes(&fs::read(authored.join("ui/game.ui")).expect("current UI")),
        migrated_ui
    );
    let migrated_resources: FileState =
        serde_json::from_value(ops[2]["state"].clone()).expect("resources state");
    let value: Value =
        serde_json::from_slice(&history.read(&migrated_resources).expect("resource bytes"))
            .expect("JSON");
    assert_eq!(value["fonts"][0], "font.ttf");
    assert_eq!(value["images"]["icon"], "icon.png");
    assert_eq!(value["click"], "click.ogg");
    assert_eq!(value["label"], "assets/Display");
    assert_eq!(
        ops[3]["state"],
        serde_json::to_value(&binary_state).expect("binary state")
    );
    assert_eq!(
        ops[4]["state"],
        serde_json::to_value(directory_state).expect("directory state")
    );
    assert_eq!(
        history.read(&current_state).expect("old blob retained"),
        current
    );
    assert_eq!(
        history
            .read(&previous_state)
            .expect("old previous retained"),
        previous
    );
    let op: Op = serde_json::from_value(ops[0].clone()).expect("file op");
    history
        .materialize(&[op.inverse()])
        .expect("undo using migrated snapshot");
    let undone = fs::read(authored.join("levels/main.scene")).expect("undo bytes");
    assert_eq!(FileState::bytes(&undone), migrated_prev);
    assert!(
        String::from_utf8(undone)
            .expect("JSON UTF8")
            .contains("@old.prefab#pf_old")
    );
    history.materialize(&[op]).expect("redo");
}

#[test]
fn undo_restores_deleted_gltf_companions_under_the_same_reference_root() {
    let root = scratch("deleted-gltf-companion");
    fs::create_dir_all(root.join("assets")).expect("assets");
    fs::write(root.join("ara.project.json"), "{}").expect("marker");
    let current = br#"{"buffers":[{"uri":"current.bin"}]}"#;
    let previous = br#"{"buffers":[{"uri":"old.bin"}]}"#;
    fs::write(root.join("assets/model.gltf"), current).expect("current model");
    fs::write(root.join("assets/current.bin"), b"current").expect("current companion");
    let history = FileHistory::new(&root);
    let current_state = history.put(current).expect("current snapshot");
    let previous_state = history.put(previous).expect("previous snapshot");
    let companion_state = history
        .put(b"old companion")
        .expect("deleted companion snapshot");
    let mut deletion = file("assets/old.bin", &companion_state, Some(&companion_state));
    deletion["state"] = Value::Null;
    journal(
        &root,
        &[
            json!({"t":"txn","txn":{"ops":[file("assets/model.gltf",&current_state,Some(&previous_state)),deletion]}}),
        ],
    );
    Migration::plan(&root, None)
        .expect("plan")
        .apply()
        .expect("migrate");
    let entry: Value = serde_json::from_str(
        fs::read_to_string(root.join(".ara/journal"))
            .expect("journal")
            .lines()
            .next()
            .expect("entry"),
    )
    .expect("JSON");
    let ops = entry["txn"]["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .map(|op| serde_json::from_value::<Op>(op.clone()).expect("file op"))
        .collect::<Vec<_>>();
    history
        .materialize(&ops.iter().rev().map(Op::inverse).collect::<Vec<_>>())
        .expect("undo");
    let authored = root.join(project_layout::AUTHORING_ROOT);
    let restored: Value =
        serde_json::from_slice(&fs::read(authored.join("model.gltf")).expect("restored model"))
            .expect("glTF");
    let uri = restored["buffers"][0]["uri"]
        .as_str()
        .expect("companion URI");
    assert_eq!(uri, "old.bin");
    assert_eq!(
        fs::read(authored.join(uri)).expect("restored companion"),
        b"old companion"
    );
}

#[test]
fn invalid_missing_and_corrupt_snapshots_fail_planning_without_writes_or_moves() {
    for kind in ["invalid", "missing", "corrupt"] {
        let root = scratch(kind);
        fs::create_dir_all(root.join("assets")).expect("assets");
        fs::write(root.join("ara.project.json"), "{}").expect("marker");
        fs::write(root.join("assets/kept.bin"), b"source").expect("source");
        let history = FileHistory::new(&root);
        let mut state = FileState::bytes(b"snapshot");
        if kind == "invalid" {
            state.hash = "invalid".into();
        }
        if kind == "corrupt" {
            state = history.put(b"snapshot").expect("fixture snapshot");
            fs::write(root.join(".ara/file-history").join(&state.hash), b"corrupt")
                .expect("fixture corruption");
        }
        journal(
            &root,
            &[json!({"t":"txn","txn":{"ops":[file("assets/kept.bin",&state,None)]}})],
        );
        let before = fs::read(root.join(".ara/journal")).expect("journal");
        assert!(
            Migration::plan(&root, None).is_err(),
            "{kind} history must fail before apply"
        );
        assert_eq!(
            fs::read(root.join(".ara/journal")).expect("unchanged journal"),
            before
        );
        assert!(!root.join(".ara/lock").exists());
        assert!(!root.join(project_layout::AUTHORING_ROOT).exists());
        assert_eq!(
            fs::read(root.join("assets/kept.bin")).expect("unmoved source"),
            b"source"
        );
        if kind != "corrupt" {
            assert!(!root.join(".ara/file-history").exists());
        }
    }
}
