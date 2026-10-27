//! The offline migration preserves document identity and native reference round trips.
use ara_host::{Project, project_layout::Migration};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn scratch(name: &str) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let path = target.join("project-layout-tests").join(format!(
        "{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).expect("scratch");
    path
}

#[test]
fn authored_index_classifies_policy_profiles_and_nested_content_consistently() {
    use ara_host::project::{AuthoredRole, authored_files};
    let root = scratch("index");
    for path in [
        "game/target/kept.scene",
        "settings/project.json",
        "settings/builds/windows.export.json",
        "game/scripts/game/game.module.json",
        "game/scripts/game/Main.cs",
        "game/models/box.glb.import.json",
        "builds/old/copied.scene",
        ".ara/cache/copied.scene",
        "target/copied.scene",
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        fs::write(path, "{}").expect("file");
    }
    fs::write(root.join("ara.project.json"), r#"{"layoutVersion":3}"#).expect("marker");
    let mut db = ara_schema::SchemaDb::new();
    ara_doc::register(&mut db);
    let files = authored_files(&root, &db).expect("index");
    let role = |path: &str| {
        files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.role.clone())
    };
    assert_eq!(
        role("settings/project.json"),
        Some(AuthoredRole::Preferences)
    );
    assert_eq!(
        role("settings/builds/windows.export.json"),
        Some(AuthoredRole::BuildPreset)
    );
    assert_eq!(
        role("scripts/game/game.module.json"),
        Some(AuthoredRole::ScriptModule)
    );
    assert_eq!(role("scripts/game/Main.cs"), Some(AuthoredRole::Script));
    assert_eq!(
        role("models/box.glb.import.json"),
        Some(AuthoredRole::ImportSidecar)
    );
    assert_eq!(
        role("target/kept.scene"),
        Some(AuthoredRole::Document {
            name: "scene".into()
        })
    );
    assert!(role("builds/old/copied.scene").is_none());
    assert!(role(".ara/cache/copied.scene").is_none());
    assert!(role("target/copied.scene").is_none());
    assert!(
        files
            .windows(2)
            .all(|pair| pair[0].storage_path < pair[1].storage_path)
    );
}
fn copy(source: &Path, dest: &Path) {
    fs::create_dir_all(dest).expect("directory");
    for entry in fs::read_dir(source).expect("fixture") {
        let entry = entry.expect("entry");
        let path = dest.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy(&entry.path(), &path);
        } else {
            fs::copy(entry.path(), path).expect("copy");
        }
    }
}

#[test]
fn migrated_project_keeps_ids_references_and_round_trips() {
    let root = scratch("roundtrip").join("game");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project"),
        &root,
    );
    fs::write(root.join("ara.project.json"),r#"{"name":"sample","main":"levels/arena.scene","rendering":{"quality":"high"},"settings":{"play.mode":"host","customPolicy":{"keep":["authored"]}},"layers":{"names":["world","ui"]},"physics":{"gravity":[0,-9.81,0]},"input":{"actions":[]},"look":{"default":"@levels/arena.scene#sc_arena001"}}"#).expect("marker");
    fs::create_dir_all(root.join("evidence")).expect("evidence");
    fs::write(root.join("evidence/proof.txt"), "kept").expect("proof");
    let before = fs::read(root.join("levels/arena.scene")).expect("scene");
    let migration = Migration::plan(&root, None).expect("plan");
    assert_eq!(
        fs::read(root.join("levels/arena.scene")).expect("dry run leaves scene"),
        before
    );
    assert!(
        !root.join(".ara").exists(),
        "planning must not even create a lock"
    );
    migration.apply().expect("apply");
    assert!(!root.join("levels").exists());
    assert!(!root.join("evidence").exists());
    assert_eq!(
        fs::read_to_string(migration.support.join("evidence/proof.txt"))
            .expect("retained evidence"),
        "kept"
    );
    assert!(root.join("settings/builds/windows.export.json").is_file());
    let preferences = ara_host::project_layout::preferences(&root).expect("preferences");
    assert_eq!(preferences["rendering"]["quality"], "high");
    assert_eq!(preferences["settings"]["play.mode"], "host");
    assert_eq!(
        preferences["settings"]["customPolicy"]["keep"],
        serde_json::json!(["authored"])
    );
    assert_eq!(
        preferences["layers"]["names"],
        serde_json::json!(["world", "ui"])
    );
    assert_eq!(
        preferences["look"]["default"],
        "@levels/arena.scene#sc_arena001"
    );
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("ara.project.json")).expect("marker"))
            .expect("JSON");
    assert!(marker.get("rendering").is_none());
    assert!(marker.get("layers").is_none());
    let project = Project::open(&root).expect("migrated project");
    assert_eq!(project.main_scene().as_deref(), Some("levels/arena.scene"));
    let (workspace, diagnostics) = project.load(&[]);
    assert!(
        !diagnostics
            .values()
            .flatten()
            .any(ara_diag::Diagnostic::is_error),
        "{diagnostics:?}"
    );
    for (path, document) in workspace.docs() {
        let written = project.write_document(document).expect("write");
        let read = project.read_document(&written, path);
        assert!(
            !read.diags.iter().any(ara_diag::Diagnostic::is_error),
            "{path}: {:?}",
            read.diags
        );
        let again = project
            .write_document(&read.doc.expect("roundtrip document"))
            .expect("write again");
        assert_eq!(written, again, "{path}");
    }
    let scene = fs::read_to_string(root.join("game/levels/arena.scene")).expect("scene");
    assert!(scene.contains("@prefabs/"), "{scene}");
    assert!(
        Migration::plan(&root, None)
            .expect("already migrated")
            .operations
            .is_empty()
    );
    let mut host = ara_host::Host::open(project, ara_host::HostConfig::default()).expect("host");
    let session = host
        .call(
            ara_proto::ConnId::LOCAL,
            "session.begin",
            serde_json::json!({"client":"migration-test","author":"human"}),
        )
        .expect("begin")["session"]
        .clone();
    host.call(ara_proto::ConnId::LOCAL,"cmd.apply",serde_json::json!({"session":session,"doc":"settings/project.json","why":"Change the project default after migration","commands":[{"cmd":"property.set","args":{"entity":"p_settings","path":"settings.rendering.quality","value":"medium"}}]})).expect("typed preferences edit");
    host.flush();
    drop(host);
    assert_eq!(
        ara_host::project_layout::preferences(&root).expect("saved preferences")["rendering"]["quality"],
        "medium"
    );
    let reloaded = Project::open(&root).expect("reloaded");
    let (workspace, _) = reloaded.load(&[]);
    assert_eq!(
        ara_host::project_layout::policy(&workspace, "rendering").expect("policy")["quality"],
        "medium"
    );
}

#[test]
fn collisions_and_live_authorities_leave_authored_files_intact() {
    let root = scratch("collision");
    fs::write(root.join("ara.project.json"), "{}").expect("marker");
    fs::create_dir_all(root.join("levels")).expect("old content");
    fs::create_dir_all(root.join("game/levels")).expect("collision");
    fs::write(root.join("levels/main.scene"), "legacy").expect("source");
    fs::write(root.join("game/levels/main.scene"), "current").expect("target");
    assert!(Migration::plan(&root, None).is_err());
    fs::remove_file(root.join("game/levels/main.scene")).expect("remove collision file");
    fs::remove_dir(root.join("game/levels")).expect("empty collision");
    fs::create_dir_all(root.join(".ara")).expect("state");
    let lock = fs::File::create(root.join(".ara/lock")).expect("lock");
    lock.try_lock().expect("authority");
    assert!(
        Migration::plan(&root, None)
            .expect("plan is read only")
            .apply()
            .is_err()
    );
    assert!(root.join("levels").is_dir());
}

#[test]
fn text_markup_and_journal_references_move_with_content() {
    let root = scratch("journal");
    fs::write(
        root.join("ara.project.json"),
        r#"{"main":"levels/main.scene"}"#,
    )
    .expect("marker");
    fs::create_dir_all(root.join("levels")).expect("levels");
    fs::create_dir_all(root.join("ui")).expect("ui");
    fs::create_dir_all(root.join(".ara")).expect("state");
    let scene = "{\"id\":\"sc_main\",\"records\":{}}\n";
    fs::write(root.join("levels/main.scene"), scene).expect("scene");
    fs::write(
        root.join("ui/menu.ui"),
        r#"<ui id="ui_menu"><image id="logo" src="@assets/logo.png#as_logo" /></ui>"#,
    )
    .expect("native UI");
    fs::create_dir_all(root.join("assets")).expect("assets");
    fs::write(root.join(".ara/journal"),"{\"t\":\"txn\",\"txn\":{\"ops\":[{\"op\":\"document\",\"path\":\"levels/main.scene\",\"value\":\"@ui/menu.ui#ui_menu\"}]}}\n").expect("journal");
    Migration::plan(&root, None)
        .expect("plan")
        .apply()
        .expect("apply");
    assert!(
        fs::read_to_string(root.join("game/ui/menu.ui"))
            .expect("UI")
            .contains("@logo.png#as_logo")
    );
    assert!(
        !root.join("assets").exists(),
        "the merged empty legacy root is removed"
    );
    let journal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".ara/journal")).expect("journal"))
            .expect("one journal line");
    assert_eq!(journal["txn"]["ops"][0]["path"], "game/levels/main.scene");
    assert_eq!(journal["txn"]["ops"][0]["value"], "@ui/menu.ui#ui_menu");
}

#[test]
fn legacy_build_profiles_keep_every_authored_field_and_pin() {
    let root = scratch("presets");
    fs::write(root.join("ara.project.json"), "{}").expect("marker");
    fs::create_dir_all(root.join("presets")).expect("presets");
    let preset = r#"{"platform":"windows","assembly":"Authored","content":"assets","config":"profile","templates":[{"hash":"keep-this-pin","source":"presets/runtime.zip"}],"version":"2.4.1"}"#;
    fs::write(root.join("presets/windows.export.json"), preset).expect("preset");
    let migration = Migration::plan(&root, None).expect("plan");
    assert!(!root.join("settings").exists());
    migration.apply().expect("apply");
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("settings/builds/windows.export.json")).expect("preset"),
    )
    .expect("JSON");
    assert_eq!(value["assembly"], "Authored");
    assert_eq!(value["config"], "profile");
    assert_eq!(value["version"], "2.4.1");
    assert_eq!(value["templates"][0]["hash"], "keep-this-pin");
    assert_eq!(
        value["templates"][0]["source"],
        "settings/builds/runtime.zip"
    );
}

#[test]
fn policy_history_stays_undoable_after_marker_policy_moves() {
    let root = scratch("policy-history");
    fs::write(root.join("ara.project.json"), "{\"name\":\"History\"}\n").expect("marker");
    let mut host = ara_host::Host::open(
        Project::open(&root).expect("project"),
        ara_host::HostConfig::default(),
    )
    .expect("host");
    let session = host
        .call(
            ara_proto::ConnId::LOCAL,
            "session.begin",
            serde_json::json!({"client":"test","author":"human"}),
        )
        .expect("begin")["session"]
        .clone();
    host.call(ara_proto::ConnId::LOCAL,"cmd.apply",serde_json::json!({"session":session,"doc":"ara.project.json","why":"Author a legacy policy","commands":[{"cmd":"property.set","args":{"entity":"p_settings","path":"settings.rendering","value":{"quality":"high"}}}]})).expect("legacy policy op");
    host.flush();
    drop(host);
    Migration::plan(&root, None)
        .expect("plan")
        .apply()
        .expect("migrate");
    let mut host = ara_host::Host::open(
        Project::open(&root).expect("project"),
        ara_host::HostConfig::default(),
    )
    .expect("reopen");
    let session = host
        .call(
            ara_proto::ConnId::LOCAL,
            "session.begin",
            serde_json::json!({"client":"test","author":"human"}),
        )
        .expect("begin")["session"]
        .clone();
    host.call(
        ara_proto::ConnId::LOCAL,
        "history.undo",
        serde_json::json!({"session":session}),
    )
    .expect("undo legacy policy");
    host.flush();
    drop(host);
    assert!(
        ara_host::project_layout::preferences(&root)
            .expect("policy")
            .get("rendering")
            .is_none()
    );
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("ara.project.json")).expect("marker"))
            .expect("JSON");
    assert!(marker.get("rendering").is_none());
    assert_eq!(marker["name"], "History");
}

#[test]
fn overlapping_draft_roots_merge_and_modules_and_local_uris_migrate() {
    let root = scratch("draft-roots");
    fs::write(
        root.join("ara.project.json"),
        r#"{"name":"game","main":"content/scenes/main.scene","layoutVersion":2}"#,
    )
    .expect("marker");
    for directory in [
        "content/scenes",
        "content/assets/textures",
        "assets/models",
        "scripts/Tools",
        "content/empty",
        "settings/builds",
    ] {
        fs::create_dir_all(root.join(directory)).expect("folder");
    }
    fs::write(
        root.join("content/scenes/main.scene"),
        r#"{"id":"sc_main","records":{},"next":"@assets/models/box.gltf#as_box"}"#,
    )
    .expect("scene");
    fs::write(root.join("content/assets/textures/pixel.png"), [1, 2, 3]).expect("texture");
    fs::write(root.join("assets/models/box.gltf"),r#"{"asset":{"version":"2.0"},"images":[{"uri":"../../content/assets/textures/pixel.png"}]}"#).expect("model");
    fs::write(
        root.join("scripts/Tools/ara.module.toml"),
        "[module]\nname='Tools'\nreferences=['Core']\nhot=true\nprofile='editor'\n",
    )
    .expect("module");
    fs::write(root.join("scripts/Tools/Tool.cs"), "class Tool {}\n").expect("script");
    fs::write(root.join("scripts/Tools/Tools.types.json"), "{}").expect("generated");
    fs::write(root.join("settings/builds/custom.export.json"),r#"{"content":"content","assembly":"Authored","icon":"content/assets/textures/pixel.png","scriptProject":"scripts/Tools","version":"3.2.1","templates":[]}"#).expect("preset");
    let migration = Migration::plan(&root, None).expect("dry run");
    assert!(!root.join(".ara").exists());
    assert!(root.join("content/scenes/main.scene").exists());
    migration.apply().expect("migrate");
    let authored = root.join(ara_host::project_layout::AUTHORING_ROOT);
    assert!(authored.join("empty").is_dir());
    assert!(authored.join("editor/scripts/Tools/Tool.cs").is_file());
    let module: serde_json::Value = serde_json::from_slice(
        &fs::read(authored.join("editor/scripts/Tools/tools.module.json")).expect("module"),
    )
    .expect("JSON");
    assert_eq!(
        module,
        serde_json::json!({"name":"Tools","references":["Core"],"hot":true})
    );
    assert!(root.join(".ara/scripts/Tools/Tools.types.json").is_file());
    let model: serde_json::Value =
        serde_json::from_slice(&fs::read(authored.join("models/box.gltf")).expect("model"))
            .expect("JSON");
    assert_eq!(model["images"][0]["uri"], "../textures/pixel.png");
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("ara.project.json")).expect("marker"))
            .expect("JSON");
    assert_eq!(
        marker["name"], "game",
        "directory names must not rewrite unrelated values"
    );
    assert_eq!(marker["main"], "scenes/main.scene");
    let preset: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("settings/builds/custom.export.json")).expect("preset"),
    )
    .expect("JSON");
    assert_eq!(preset["content"], ara_host::project_layout::AUTHORING_ROOT);
    assert_eq!(preset["icon"], "textures/pixel.png");
    assert_eq!(preset["scriptProject"], "editor/scripts/Tools");
    assert_eq!(preset["version"], "3.2.1");
    assert!(
        Migration::plan(&root, None)
            .expect("idempotent")
            .operations
            .is_empty()
    );
}

#[test]
fn authored_reference_paths_and_project_preferences_do_not_share_identity() {
    use ara_host::project::{AuthoredScope, authored_files};
    let root = scratch("path-bases");
    fs::write(
        root.join("ara.project.json"),
        r#"{"layoutVersion":3,"main":"scenes/main.scene"}"#,
    )
    .expect("marker");
    ara_host::project_layout::write_support_files(&root, "Paths").expect("scaffold");
    let authored = root.join(ara_host::project_layout::AUTHORING_ROOT);
    fs::create_dir_all(authored.join("settings")).expect("authored folder");
    fs::write(
        authored.join("settings/project.json"),
        r#"{"ordinary":"authored file"}"#,
    )
    .expect("source");
    fs::write(
        authored.join("scenes/main.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{}}"#,
    )
    .expect("scene");
    fs::create_dir_all(root.join("packages/future")).expect("packages");
    fs::write(root.join("packages/future/fake.scene"), "{}").expect("excluded");
    let project = Project::open(&root).expect("open");
    let files = authored_files(&root, &project.db).expect("index");
    assert_eq!(
        files
            .iter()
            .filter(|file| file.path == "settings/project.json")
            .count(),
        2
    );
    assert!(
        files
            .iter()
            .any(|file| file.path == "settings/project.json"
                && file.scope == AuthoredScope::Authored)
    );
    assert!(
        !files
            .iter()
            .any(|file| file.storage_path.starts_with("packages/"))
    );
    let (workspace, _) = project.load(&[]);
    assert!(workspace.doc("pj_project").is_some());
    assert!(workspace.doc("pj_preferences").is_some());
    assert_eq!(
        workspace.index().path_of("sc_main"),
        Some("scenes/main.scene")
    );
    assert_eq!(
        workspace.path_of("sc_main"),
        Some(
            format!(
                "{}/scenes/main.scene",
                ara_host::project_layout::AUTHORING_ROOT
            )
            .as_str()
        )
    );
    assert!(workspace.index().path_of("pj_preferences").is_none());
    let mut host = ara_host::Host::open(project, ara_host::HostConfig::default()).expect("host");
    let session = host
        .call(
            ara_proto::ConnId::LOCAL,
            "session.begin",
            serde_json::json!({"client":"paths","author":"human"}),
        )
        .expect("session")["session"]
        .clone();
    host.call(ara_proto::ConnId::LOCAL,"cmd.apply",serde_json::json!({"session":session,"doc":"ara.project.json","why":"Create an authored scene and select it","commands":[
        {"cmd":"doc.create","args":{"path":"scenes/other.scene","kind":"scene","id":"sc_other"}},
        {"cmd":"project.setMain","args":{"scene":"@scenes/other.scene#sc_other"}}
    ]})).expect("typed create");
    host.flush();
    drop(host);
    assert!(authored.join("scenes/other.scene").is_file());
    let reloaded = Project::open(&root).expect("reload");
    assert_eq!(reloaded.main_scene().as_deref(), Some("scenes/other.scene"));
    assert_eq!(
        reloaded.index.path_of("sc_other"),
        Some("scenes/other.scene")
    );
}

#[test]
fn imported_source_inside_a_same_named_feature_folder_keeps_its_reference_path() {
    let root = scratch("nested-root-name");
    fs::write(root.join("ara.project.json"), r#"{"layoutVersion":3}"#).expect("marker");
    let prefix = ara_doc::AUTHORING_ROOT;
    let source = format!("{prefix}/pixel.bin");
    let storage = format!("{prefix}/{source}");
    let file = root.join(&storage);
    fs::create_dir_all(file.parent().expect("feature folder")).expect("directory");
    fs::write(&file, b"source").expect("source");
    let mut db = ara_schema::SchemaDb::new();
    ara_asset::register_schema(&mut db);
    let sidecar = ara_doc::read(
        r#"{"$schema":"ara://schema/import/1","id":"as_nested","records":{"i_settings":{"importer":"test","version":"1"}}}"#,
        &format!("{storage}.import.json"), &db, None,
    ).doc.expect("valid sidecar");
    fs::write(
        root.join(format!("{storage}.import.json")),
        ara_doc::write(&sidecar, &db),
    )
    .expect("canonical sidecar");
    let project = Project::open_with(&root, |db, _| {
        ara_asset::register_schema(db);
        Vec::new()
    })
    .expect("project");
    assert_eq!(project.index.path_of("as_nested"), Some(source.as_str()));
    let host = ara_host::Host::open(project, ara_host::HostConfig::default()).expect("host");
    assert_eq!(
        host.store().workspace().index().path_of("as_nested"),
        Some(source.as_str())
    );
    assert_eq!(
        host.store().workspace().path_of("as_nested"),
        Some(format!("{storage}.import.json").as_str())
    );
}

#[test]
fn import_policy_scope_does_not_collide_with_same_named_authored_document() {
    let root = scratch("import-policy-scope");
    fs::write(root.join("ara.project.json"), r#"{"layoutVersion":3}"#).expect("marker");
    let policy = ara_asset::ProjectImportSettings::default()
        .canonical_text()
        .expect("policy");
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    for directory in [root.join("settings"), authored.join("settings")] {
        fs::create_dir_all(directory).expect("directory");
    }
    fs::write(root.join(ara_asset::PROJECT_IMPORT_SETTINGS_FILE), &policy).expect("project policy");
    fs::write(
        authored.join("settings/ara.imports"),
        policy.replace("ip_project", "ip_feature"),
    )
    .expect("authored imports");
    let project = Project::open_with(&root, |db, _| {
        ara_asset::register_schema(db);
        Vec::new()
    })
    .expect("open");
    assert!(project.index.path_of("ip_project").is_none());
    assert_eq!(
        project.index.path_of("ip_feature"),
        Some("settings/ara.imports")
    );
    let (mut workspace, _) = project.load(&[]);
    assert!(workspace.is_project_document("ip_project"));
    assert!(!workspace.is_project_document("ip_feature"));
    assert_eq!(
        workspace
            .find("@settings/ara.imports#ip_feature")
            .map(ToString::to_string),
        Some("ip_feature".into())
    );
    let policy = workspace
        .doc("ip_project")
        .expect("transactional policy")
        .clone();
    workspace.insert("settings/ara.imports", policy);
    assert!(
        workspace.is_project_document("ip_project"),
        "reload preserves explicit project scope"
    );
    assert_eq!(
        workspace.index().id_of("settings/ara.imports"),
        Some("ip_feature")
    );
}

#[test]
fn migration_keeps_freely_named_nested_derived_folders() {
    let root = scratch("authored-target");
    fs::write(
        root.join("ara.project.json"),
        r#"{"main":"content/target/main.scene","layoutVersion":2}"#,
    )
    .expect("marker");
    for directory in ["content/target", "assets/node_modules", "target"] {
        fs::create_dir_all(root.join(directory)).expect("folder");
    }
    fs::write(
        root.join("content/target/main.scene"),
        r#"{"id":"sc_main","records":{}}"#,
    )
    .expect("authored scene");
    fs::write(root.join("assets/node_modules/kept.txt"), "authored source").expect("authored file");
    fs::write(root.join("target/local.txt"), "derived").expect("derived file");
    Migration::plan(&root, None)
        .expect("plan")
        .apply()
        .expect("migrate");
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    assert!(authored.join("target/main.scene").is_file());
    assert_eq!(
        fs::read_to_string(authored.join("node_modules/kept.txt")).expect("authored file"),
        "authored source"
    );
    assert_eq!(
        fs::read_to_string(root.join("target/local.txt")).expect("derived file"),
        "derived"
    );
}

#[test]
fn migration_pins_legacy_ui_identity_before_storage_path_changes() {
    let root = scratch("ui-identity");
    fs::create_dir_all(root.join("content/ui")).expect("folder");
    fs::write(root.join("ara.project.json"), r#"{"layoutVersion":2}"#).expect("marker");
    fs::write(root.join("content/ui/game.ui"), "<ui id='root'/>").expect("legacy UI");
    let old = Project::open(&root)
        .expect("old project")
        .index
        .id_of("content/ui/game.ui")
        .expect("old UI identity")
        .to_owned();
    fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&serde_json::json!({"layoutVersion":2,"uiReference":format!("@content/ui/game.ui#{old}")}))).expect("reference");
    Migration::plan(&root, None)
        .expect("plan")
        .apply()
        .expect("migrate");
    let project = Project::open(&root).expect("migrated project");
    assert_eq!(project.index.id_of("ui/game.ui"), Some(old.as_str()));
    let source =
        fs::read_to_string(root.join(ara_doc::AUTHORING_ROOT).join("ui/game.ui")).expect("source");
    assert!(source.contains(&format!("<!-- ara:asset {old} -->")));
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("ara.project.json")).expect("marker"))
            .expect("JSON");
    assert_eq!(marker["uiReference"], format!("@ui/game.ui#{old}"));
}

#[test]
fn outpost_native_ui_preserves_its_pristine_legacy_identity() {
    const BASELINE_ID: &str = "ui_6fc175af148d8e6a";
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/outpost");
    let pinned = fs::read_to_string(sample.join(ara_doc::AUTHORING_ROOT).join("ui/game.ui"))
        .expect("checked-in Outpost UI");
    let header = format!("<!-- ara:asset {BASELINE_ID} -->\n");
    let legacy = pinned
        .strip_prefix(&header)
        .expect("sample pins its original ui/game.ui identity");
    let root = scratch("outpost-ui-baseline");
    fs::create_dir_all(root.join("ui")).expect("legacy folder");
    fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&serde_json::json!({"name":"Outpost","uiReference":format!("@ui/game.ui#{BASELINE_ID}")}))).expect("legacy marker");
    fs::write(root.join("ui/game.ui"), legacy).expect("pristine headerless UI");
    for (file, text) in [
        ("game.css", ""),
        ("bindings.json", "[]"),
        ("resources.json", "{}"),
    ] {
        fs::write(root.join("ui").join(file), text).expect("bootstrap source");
    }
    assert_eq!(
        Project::open(&root)
            .expect("legacy project")
            .index
            .id_of("ui/game.ui"),
        Some(BASELINE_ID)
    );
    Migration::plan(&root, None)
        .expect("plan")
        .apply()
        .expect("migrate");
    let project = Project::open(&root).expect("migrated project");
    assert_eq!(project.index.id_of("ui/game.ui"), Some(BASELINE_ID));
    assert_eq!(
        fs::read_to_string(root.join(ara_doc::AUTHORING_ROOT).join("ui/game.ui"))
            .expect("migrated UI"),
        pinned
    );
    let policy = ara_host::project_layout::preferences(&root).expect("UI policy");
    assert_eq!(
        policy["ui"],
        serde_json::json!({"main":"ui/game.ui","stylesheet":"ui/game.css","bindings":"ui/bindings.json","resources":"ui/resources.json"})
    );
    assert!(
        Migration::plan(&root, None)
            .expect("round trip")
            .operations
            .is_empty()
    );
}

#[test]
fn migration_repairs_known_paths_and_references_without_renaming_labels() {
    let root = scratch("reference-owners");
    for folder in ["levels", "assets", "ui"] {
        fs::create_dir_all(root.join(folder)).expect("folder");
    }
    fs::write(root.join("ara.project.json"), r#"{"name":"assets/Display","main":"levels/main.scene","look":{"default":"assets/day.look.json"},"ui":{"main":"ui/menu.ui","resources":"ui/resources.json"}}"#).expect("marker");
    fs::write(root.join("levels/main.scene"), r#"{"id":"sc_main","records":{"e_label":{"name":"assets/Display","description":"See assets/Display","target":"@assets/logo.png#as_logo"}}}"#).expect("scene");
    fs::write(
        root.join("ui/menu.ui"),
        r#"<ui id="ui_menu"><text id="label" text="assets/Display"/></ui>"#,
    )
    .expect("UI");
    fs::write(root.join("ui/resources.json"), r#"{"fonts":["assets/font.ttf"],"images":{"logo":"assets/logo.png"},"name":"assets/Display"}"#).expect("resources");
    for file in ["font.ttf", "logo.png"] {
        fs::write(root.join("assets").join(file), b"source").expect("media");
    }
    fs::write(root.join("assets/day.look.json"), "{}").expect("look");
    Migration::plan(&root, None)
        .expect("plan")
        .apply()
        .expect("migrate");
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    let scene: serde_json::Value =
        serde_json::from_slice(&fs::read(authored.join("levels/main.scene")).expect("scene"))
            .expect("JSON");
    assert_eq!(scene["records"]["e_label"]["name"], "assets/Display");
    assert_eq!(
        scene["records"]["e_label"]["description"],
        "See assets/Display"
    );
    assert_eq!(scene["records"]["e_label"]["target"], "@logo.png#as_logo");
    let resources: serde_json::Value =
        serde_json::from_slice(&fs::read(authored.join("ui/resources.json")).expect("resources"))
            .expect("JSON");
    assert_eq!(resources["fonts"][0], "font.ttf");
    assert_eq!(resources["images"]["logo"], "logo.png");
    assert_eq!(resources["name"], "assets/Display");
    assert!(
        fs::read_to_string(authored.join("ui/menu.ui"))
            .expect("UI")
            .contains("text=\"assets/Display\"")
    );
    assert_eq!(
        ara_host::project_layout::preferences(&root).expect("policy")["look"]["default"],
        "day.look.json"
    );
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("ara.project.json")).expect("marker"))
            .expect("JSON");
    assert_eq!(marker["name"], "assets/Display");
}

#[test]
fn migration_relocates_file_history_storage_paths_and_keeps_blob_tokens() {
    let root = scratch("file-history-paths");
    fs::create_dir_all(root.join("assets")).expect("authored folder");
    fs::write(root.join("ara.project.json"), "{}").expect("marker");
    let bytes = b"class History {}";
    fs::write(root.join("assets/History.cs"), bytes).expect("source");
    let history = ara_host::file_state::FileHistory::new(&root);
    let state = history.put(bytes).expect("immutable history");
    let entries = [
        serde_json::json!({"t":"txn","txn":{"ops":[{"op":"file","path":"assets/History.cs","state":state,"prev":null}]}}),
        serde_json::json!({"t":"fileFlush","path":"assets/History.cs","rev":1,"state":state}),
    ];
    fs::write(
        root.join(".ara/journal"),
        entries
            .iter()
            .map(|entry| serde_json::to_string(entry).expect("journal entry") + "\n")
            .collect::<String>(),
    )
    .expect("journal");
    Migration::plan(&root, None)
        .expect("plan")
        .apply()
        .expect("migrate");
    let journal = fs::read_to_string(root.join(".ara/journal")).expect("journal");
    let entries: Vec<serde_json::Value> = journal
        .lines()
        .map(|line| serde_json::from_str(line).expect("journal entry"))
        .collect();
    let expected = format!("{}/History.cs", ara_doc::AUTHORING_ROOT);
    assert_eq!(entries[0]["txn"]["ops"][0]["path"], expected);
    assert_eq!(entries[1]["path"], expected);
    assert_eq!(
        entries[0]["txn"]["ops"][0]["id"],
        ara_doc::file_state::record_id(&expected).to_string()
    );
    assert_eq!(
        entries[0]["txn"]["ops"][0]["state"],
        serde_json::to_value(&state).expect("state")
    );
    assert_eq!(history.read(&state).expect("retained history bytes"), bytes);
}

#[test]
fn modern_authored_folders_named_like_legacy_roots_keep_their_references() {
    let root = scratch("modern-root-names");
    fs::write(
        root.join("ara.project.json"),
        r#"{"layoutVersion":3,"main":"game/level.scene"}"#,
    )
    .expect("marker");
    ara_host::project_layout::write_support_files(&root, "Names").expect("support");
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    for folder in ["game", "assets", "content"] {
        fs::create_dir_all(authored.join(folder)).expect("feature");
    }
    let scene = r#"{"id":"sc_main","records":{"e_reference":{"prefab":"@game/box.prefab#pf_nested","asset":"@assets/mesh.prefab#pf_mesh","panel":"@content/panel.prefab#pf_panel"}}}"#;
    fs::write(authored.join("game/level.scene"), scene).expect("scene");
    for (path, id) in [
        ("game/box.prefab", "pf_nested"),
        ("box.prefab", "pf_root"),
        ("assets/mesh.prefab", "pf_mesh"),
        ("content/panel.prefab", "pf_panel"),
    ] {
        fs::write(
            authored.join(path),
            ara_json::to_canonical_string(&serde_json::json!({"id":id,"records":{}})),
        )
        .expect("prefab");
    }
    let plan = Migration::plan(&root, None).expect("modern plan");
    plan.apply().expect("idempotent migration");
    assert_eq!(
        fs::read_to_string(authored.join("game/level.scene")).expect("scene"),
        scene
    );
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("ara.project.json")).expect("marker"))
            .expect("JSON");
    assert_eq!(marker["main"], "game/level.scene");
    assert!(
        Migration::plan(&root, None)
            .expect("round trip")
            .operations
            .is_empty()
    );
}
