//! A preview-3 project migrates as one physical, attributed and reversible transaction.
use ara_host::{
    Host, HostConfig, Project,
    project_layout::{MigrationControl, migrate_project, migration_plan},
};
use ara_proto::ConnId;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn scratch(name: &str) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("ara-migration-tests"));
    let root = target.join("migration-v4").join(format!(
        "{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).expect("scratch");
    root
}
fn preview3() -> PathBuf {
    let root = scratch("preview3");
    for dir in ["game/scenes", "game/game", "game/ui", "settings", "tests"] {
        fs::create_dir_all(root.join(dir)).expect("directory");
    }
    fs::write(
        root.join("ara.project.json"),
        r#"{"layoutVersion":3,"name":"Preview Three","main":"scenes/main.scene"}"#,
    )
    .expect("marker");
    fs::write(root.join("game/scenes/main.scene"),r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{"e_instance":{"prefab":"@game/box.prefab#pf_box"}}}"#).expect("scene");
    fs::write(
        root.join("game/game/box.prefab"),
        r#"{"$schema":"ara://schema/prefab/1","id":"pf_box","records":{}}"#,
    )
    .expect("prefab");
    fs::write(
        root.join("game/ui/game.ui"),
        "<?xml version='1.0'?>\n<ui id='hud'/>",
    )
    .expect("native UI");
    fs::write(root.join("tests/keep.txt"), "retained user support").expect("support");
    root
}
fn await_job(host: &mut Host, session: &Value, value: Value) -> Value {
    let Some(job) = value["job"].as_str() else {
        return value;
    };
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        host.tick();
        let status = host
            .call(
                ConnId::LOCAL,
                "jobs.status",
                json!({"session":session,"job":job}),
            )
            .expect("job status");
        match status["status"].as_str() {
            Some("done") => return status["result"].clone(),
            Some("failed" | "cancelled") => panic!("job failed: {status}"),
            _ => {}
        }
        assert!(Instant::now() < until, "job timed out: {status}");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn preview3_migration_is_readonly_until_consent_and_one_undo_restores_original_bytes() {
    let root = preview3();
    let guide = "- **Layout:** `game/` holds every authored file\nPaths are relative to `game/`, not the project root.\nA feature may still be named game.\n";
    fs::write(root.join("AGENTS.md"), guide).expect("managed guide");
    let old_ui = fs::read(root.join("game/ui/game.ui")).expect("original UI");
    let old_marker = fs::read(root.join("ara.project.json")).expect("original marker");
    let old_id = ara_host::project::read_document(
        std::str::from_utf8(&old_ui).expect("UTF-8"),
        "game/ui/game.ui",
        &ara_schema::SchemaDb::new(),
        None,
    )
    .doc
    .expect("legacy native UI")
    .id;
    let plan = migration_plan(&root).expect("preview");
    assert!(plan.needs_migration);
    assert_eq!(plan.from_version, 3);
    assert_eq!(plan.to_version, 4);
    assert!(
        plan.preview_tree
            .iter()
            .any(|path| path == "content/game/box.prefab")
    );
    assert!(
        !root.join(".ara").exists(),
        "preview cannot create state or even a lock"
    );
    assert!(
        Project::open(&root).is_err(),
        "normal opens never load old roots"
    );
    let result = migrate_project(
        &root,
        &plan.plan_token,
        &MigrationControl::default(),
        &mut |_, _| {},
    )
    .expect("typed migration");
    assert_eq!(result["restartRequired"], true);
    assert!(!root.join("game").exists());
    let updated_guide = fs::read_to_string(root.join("AGENTS.md")).expect("updated guide");
    assert!(updated_guide.contains("`content/` holds every authored file"));
    assert!(updated_guide.contains("relative to `content/`"));
    assert!(updated_guide.contains("A feature may still be named game."));
    assert_eq!(
        fs::read_to_string(root.join("migration-support/tests/keep.txt")).expect("support"),
        "retained user support"
    );
    let scene = fs::read_to_string(root.join("content/scenes/main.scene")).expect("scene");
    assert!(
        scene.contains("@game/box.prefab#pf_box"),
        "a freely named game/ folder must not be stripped twice"
    );
    let project = Project::open(&root).expect("current project");
    assert_eq!(project.index.id_of("ui/game.ui"), Some(old_id.as_str()));
    let mut host = Host::open(project, HostConfig::default()).expect("normal host");
    assert_eq!(
        host.store().history().len(),
        1,
        "migration is a single undo step"
    );
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"migration-proof","author":"human"}),
        )
        .expect("session")["session"]
        .clone();
    let undo = host
        .call(ConnId::LOCAL, "history.undo", json!({"session":session}))
        .expect("undo starts");
    await_job(&mut host, &session, undo);
    host.flush();
    assert_eq!(
        fs::read(root.join("game/ui/game.ui")).expect("restored native source"),
        old_ui
    );
    assert_eq!(
        fs::read(root.join("ara.project.json")).expect("restored marker"),
        old_marker
    );
    assert!(!root.join("content").exists());
    assert_eq!(
        fs::read_to_string(root.join("AGENTS.md")).expect("restored guide"),
        guide
    );
    let blocked = host
        .call(
            ConnId::LOCAL,
            "cmd.apply",
            json!({"session":session,"commands":[]}),
        )
        .expect_err("old authoring authority is closed to normal consumers");
    assert!(
        blocked
            .diagnostics()
            .iter()
            .any(|d| d.code == ara_diag::codes::PROJECT_MIGRATION_REQUIRED)
    );
    let redo = host
        .call(ConnId::LOCAL, "history.redo", json!({"session":session}))
        .expect("redo starts");
    await_job(&mut host, &session, redo);
    host.flush();
    drop(host);
    let reopened = Host::open(
        Project::open(&root).expect("redo restores current open contract"),
        HostConfig::default(),
    )
    .expect("redo journal reopens as a normal authority");
    assert_eq!(reopened.store().history().len(), 3);
    assert_eq!(
        reopened.store().workspace().index().id_of("ui/game.ui"),
        Some(old_id.as_str())
    );
}
#[test]
fn changed_preview_collision_and_cancel_leave_source_untouched() {
    let root = preview3();
    let plan = migration_plan(&root).expect("plan");
    fs::write(root.join("game/game/new.txt"), "external change").expect("change");
    assert!(
        migrate_project(
            &root,
            &plan.plan_token,
            &MigrationControl::default(),
            &mut |_, _| {}
        )
        .is_err()
    );
    assert!(!root.join(".ara").exists());
    let plan = migration_plan(&root).expect("new plan");
    let control = MigrationControl::default();
    assert!(control.cancel());
    assert!(migrate_project(&root, &plan.plan_token, &control, &mut |_, _| {}).is_err());
    assert!(!root.join(".ara").exists());
    fs::create_dir_all(root.join("content/game")).expect("collision parent");
    fs::write(
        root.join("content/game/box.prefab"),
        "unrelated existing bytes",
    )
    .expect("collision");
    assert!(migration_plan(&root).is_err());
    assert!(root.join("game/game/box.prefab").is_file());
}

#[test]
fn prior_attributed_history_is_retained_and_requires_undoing_migration_first() {
    let root = preview3();
    let mut db = ara_schema::SchemaDb::new();
    ara_doc::register(&mut db);
    let registry = ara_cmd::Registry::builtin(&mut db);
    let marker = fs::read_to_string(root.join("ara.project.json")).expect("marker");
    let doc = ara_host::project::read_document(&marker, "ara.project.json", &db, None)
        .doc
        .expect("metadata");
    let mut workspace = ara_doc::Workspace::default();
    workspace.insert_project_document("ara.project.json", doc);
    let mut store = ara_ops::Store::new(db, workspace);
    let session = ara_cmd::Session::human();
    let result=registry.apply(&mut store,&ara_cmd::Batch::new("pj_project",[("property.set",json!({"entity":"p_settings","path":"settings.name","value":"Authored before migration"}))]),&session).expect("prior typed edit");
    let prior = result.txn.expect("prior transaction");
    let txn = store.txn(&prior).expect("stored transaction").clone();
    let text = ara_host::project::write_document(
        store.workspace().doc("pj_project").expect("marker"),
        store.schema(),
    )
    .expect("native codec");
    fs::write(root.join("ara.project.json"), &text).expect("legacy file");
    let (mut journal, _, _) =
        ara_host::journal::Journal::open(&root.join(".ara")).expect("legacy journal");
    journal
        .append(&ara_host::journal::Entry::Txn {
            untracked: false,
            session: None,
            txn,
        })
        .expect("attributed prior transaction");
    journal
        .append(&ara_host::journal::Entry::Flush {
            doc: "pj_project".into(),
            rev: 1,
            hash: ara_host::journal::hash(&text),
        })
        .expect("legacy native baseline");
    drop(journal);
    let old_journal = fs::read(root.join(".ara/journal")).expect("existing history");
    let plan = migration_plan(&root).expect("preview");
    migrate_project(
        &root,
        &plan.plan_token,
        &MigrationControl::default(),
        &mut |_, _| {},
    )
    .expect("migration");
    assert!(
        fs::read(root.join(".ara/journal"))
            .expect("journal")
            .starts_with(&old_journal),
        "history entries are never rewritten to disguise old paths"
    );
    let mut host = Host::open(
        Project::open(&root).expect("current project"),
        HostConfig::default(),
    )
    .expect("host");
    assert_eq!(host.store().history().len(), 2);
    assert_eq!(host.store().history()[0].id, prior);
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"test","author":"human"}),
        )
        .expect("session")["session"]
        .clone();
    let error = host
        .call(
            ConnId::LOCAL,
            "history.undo",
            json!({"session":session,"txn":prior}),
        )
        .expect_err("cannot replay old path semantics under current root");
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code == ara_diag::codes::STALE_BASE)
    );
    let undo = host
        .call(ConnId::LOCAL, "history.undo", json!({"session":session}))
        .expect("undo migration");
    await_job(&mut host, &session, undo);
    host.flush();
    drop(host);
    assert_eq!(
        fs::read_to_string(root.join("ara.project.json")).expect("old marker"),
        text
    );
    assert!(Project::open(&root).is_err());
}

#[test]
fn corrupt_history_fails_readonly_before_source_movement() {
    let root = preview3();
    fs::create_dir_all(root.join(".ara")).expect("state");
    fs::write(root.join(".ara/journal"), "{broken journal\n").expect("corrupt history fixture");
    let error = migration_plan(&root).expect_err("cannot silently discard historical transactions");
    assert!(
        error
            .iter()
            .any(|d| d.code == ara_diag::codes::JOURNAL_CORRUPT)
    );
    assert!(!root.join("content").exists());
    assert!(root.join("game/scenes/main.scene").exists());
}

#[test]
fn preview3_directory_policy_paths_follow_converted_editor_modules() {
    let root = preview3();
    fs::create_dir_all(root.join("game/scripts/Tools")).expect("legacy editor module");
    fs::create_dir_all(root.join("settings/builds")).expect("profiles");
    fs::write(
        root.join("game/scripts/Tools/ara.module.toml"),
        "[module]\nname='Tools'\nprofile='editor'\nhot=true\n",
    )
    .expect("legacy manifest");
    fs::write(root.join("game/scripts/Tools/Tool.cs"), "class Tool {}\n").expect("source");
    fs::write(root.join("settings/builds/tools.export.json"),r#"{"content":"game","scriptProject":"scripts/Tools","name":"Tools","version":"2.0.0","templates":[]}"#).expect("authored preset");
    let plan = migration_plan(&root).expect("reviewed plan");
    migrate_project(
        &root,
        &plan.plan_token,
        &MigrationControl::default(),
        &mut |_, _| {},
    )
    .expect("migrate");
    let preset: Value = serde_json::from_slice(
        &fs::read(root.join("settings/builds/tools.export.json")).expect("preset"),
    )
    .expect("JSON");
    assert_eq!(preset["scriptProject"], "editor/scripts/Tools");
    assert_eq!(preset["version"], "2.0.0");
    assert!(
        root.join("content/editor/scripts/Tools/tools.module.json")
            .is_file()
    );
    assert!(
        fs::read_to_string(root.join("content/scenes/main.scene"))
            .expect("scene")
            .contains("@game/box.prefab#pf_box")
    );
}
