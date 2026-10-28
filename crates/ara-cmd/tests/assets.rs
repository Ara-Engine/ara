//! Asset operations share the normal transaction, index, dry-run and undo contracts.
use ara_cmd::builtin::{AssetConsumer, AssetLocalInput, AssetMetadata, preview_assets};
use ara_cmd::{Batch, Call, Registry as KernelRegistry, Session};
use ara_doc::file_state::{FileState, PreparedInventory};
use ara_doc::{AssetId, Document, IdGen, RecordId, Workspace};
use ara_ops::{Author, Store};
use serde_json::{Value, json};

struct Registry(KernelRegistry);
impl Registry {
    fn builtin(db: &mut ara_schema::SchemaDb) -> Self {
        Self(KernelRegistry::builtin(db))
    }
    fn apply(
        &self,
        store: &mut Store,
        batch: &Batch,
        session: &Session,
    ) -> Result<ara_cmd::BatchResult, Vec<ara_diag::Diagnostic>> {
        let mut metadata = AssetMetadata::new("asset-operation-test-inventory".into());
        metadata.inventory.complete = true;
        self.apply_with_metadata(store, batch, session, metadata)
    }
    fn apply_with_metadata(
        &self,
        store: &mut Store,
        batch: &Batch,
        session: &Session,
        metadata: AssetMetadata,
    ) -> Result<ara_cmd::BatchResult, Vec<ara_diag::Diagnostic>> {
        store.clear_prepared_files();
        let mut prepared = PreparedInventory {
            entries: store
                .workspace()
                .file_entries()
                .map(|(path, state)| (path.to_owned(), Some(state.clone())))
                .collect(),
            metadata: serde_json::to_value(metadata).expect("metadata"),
            ..PreparedInventory::default()
        };
        let preview = preview_assets(store.workspace(), store.schema(), batch, &mut prepared)?;
        let mut metadata: AssetMetadata =
            serde_json::from_value(prepared.metadata.clone()).expect("preview metadata");
        for document in preview.documents {
            metadata
                .operations
                .get_mut(&document.operation_key)
                .expect("operation")
                .document_states
                .insert(
                    document.path,
                    FileState::bytes(ara_doc::write(&document.document, store.schema()).as_bytes()),
                );
        }
        prepared.metadata = serde_json::to_value(metadata).expect("prepared metadata");
        preview_assets(store.workspace(), store.schema(), batch, &mut prepared)?;
        store
            .prepare_files(prepared)
            .map_err(|diagnostic| vec![diagnostic])?;
        let result = self.0.apply(store, batch, session);
        store.clear_prepared_files();
        result
    }
}

fn document(kind: &str, id: &str, records: Value) -> Document {
    let mut document = Document::new(kind, AssetId::parse(id).expect("asset ID"));
    for (id, record) in records.as_object().expect("records") {
        document.insert_unchecked(
            RecordId::parse(id).expect("record ID"),
            record.as_object().expect("record").clone(),
        );
    }
    document
}
fn setup() -> (Registry, Store) {
    let mut db = ara_schema::SchemaDb::new();
    ara_doc::register(&mut db);
    let registry = Registry::builtin(&mut db);
    let mut workspace = Workspace::default();
    workspace.insert(
        "ara.project.json",
        document(
            "project",
            "pj_project",
            json!({"p_settings":{"settings":{"name":"Asset operations"}}}),
        ),
    );
    workspace.insert("content/props/box.prefab",document("prefab","pf_box",json!({"e_root":{"name":"Box","order":"a0"},"e_lid":{"name":"Lid","parent":"e_root","order":"a0"}})));
    workspace.insert("content/main.scene",document("scene","sc_main",json!({"e_instance":{"prefab":"@content/props/box.prefab#pf_box","name":"Box instance"}})));
    workspace.observe_file("content/props".into(), Some(FileState::directory()));
    workspace.observe_file(
        "content/props/box.prefab".into(),
        Some(FileState::bytes(b"a")),
    );
    (registry, Store::with_ids(db, workspace, IdGen::seeded(813)))
}
fn batch(command: &str, args: Value, dry_run: bool) -> Batch {
    let mut batch = Batch::new("pj_project", []);
    batch.calls.push(Call {
        command: command.into(),
        params: args,
    });
    batch.dry_run = dry_run;
    batch.why = Some("Exercise real asset organization".into());
    batch
}

#[test]
fn move_keeps_ids_repairs_references_and_is_one_undo_step() {
    let (registry, mut store) = setup();
    let args =
        json!({"source":"content/props/box.prefab","destination":"content/warehouse/box.prefab"});
    let preview = registry
        .apply(
            &mut store,
            &batch("asset.move", args.clone(), true),
            &Session::human(),
        )
        .expect("dry run");
    assert_eq!(store.revision(), 0);
    assert!(store.workspace().find("content/props/box.prefab").is_some());
    let result = registry
        .apply(
            &mut store,
            &batch("asset.move", args, false),
            &Session::human(),
        )
        .expect("move");
    assert_eq!(preview.ops, result.ops);
    assert_eq!(store.history().len(), 1);
    assert_eq!(
        store
            .workspace()
            .find("content/warehouse/box.prefab")
            .map(ToString::to_string)
            .as_deref(),
        Some("pf_box")
    );
    assert!(store.workspace().find("content/props/box.prefab").is_none());
    assert_eq!(
        store
            .workspace()
            .doc("sc_main")
            .expect("scene")
            .record("e_instance")
            .expect("instance")["prefab"],
        "@content/warehouse/box.prefab#pf_box"
    );
    store.undo_last(&Author::Human).expect("one undo");
    assert!(store.workspace().find("content/props/box.prefab").is_some());
    assert_eq!(
        store
            .workspace()
            .doc("sc_main")
            .expect("scene")
            .record("e_instance")
            .expect("instance")["prefab"],
        "@content/props/box.prefab#pf_box"
    );
    store.redo_last(&Author::Human).expect("one redo");
    assert!(
        store
            .workspace()
            .find("content/warehouse/box.prefab")
            .is_some()
    );
}

#[test]
fn duplicate_mints_asset_and_entity_ids_but_keeps_the_original_unchanged() {
    let (registry, mut store) = setup();
    let original = store.workspace().doc("pf_box").expect("original").clone();
    let args =
        json!({"source":"content/props/box.prefab","destination":"content/props/box-copy.prefab"});
    let preview = registry
        .apply(
            &mut store,
            &batch("asset.duplicate", args.clone(), true),
            &Session::human(),
        )
        .expect("preview");
    let result = registry
        .apply(
            &mut store,
            &batch("asset.duplicate", args, false),
            &Session::human(),
        )
        .expect("duplicate");
    assert_eq!(preview.outputs, result.outputs);
    let id = store
        .workspace()
        .find("content/props/box-copy.prefab")
        .expect("copy");
    assert_ne!(id.as_str(), "pf_box");
    let duplicate = store.workspace().doc(id.as_str()).expect("copy");
    assert_eq!(duplicate.records().len(), 2);
    assert!(
        duplicate
            .records()
            .keys()
            .all(|id| !original.records().contains_key(id))
    );
    let root = duplicate
        .records()
        .iter()
        .find(|(_, record)| record["name"] == "Box")
        .expect("copied root")
        .0;
    let lid = duplicate
        .records()
        .values()
        .find(|record| record["name"] == "Lid")
        .expect("copied lid");
    assert_eq!(lid["parent"], root.to_string());
    assert_eq!(
        store.workspace().doc("pf_box").expect("original"),
        &original
    );
    store.undo_last(&Author::Human).expect("undo duplicate");
    assert!(
        store
            .workspace()
            .find("content/props/box-copy.prefab")
            .is_none()
    );
    assert_eq!(
        store.workspace().doc("pf_box").expect("original"),
        &original
    );
}

#[test]
fn delete_reports_inbound_owners_and_collision_failure_is_atomic() {
    let (registry, mut store) = setup();
    let error = registry
        .apply(
            &mut store,
            &batch(
                "asset.delete",
                json!({"source":"content/props/box.prefab"}),
                false,
            ),
            &Session::human(),
        )
        .expect_err("inbound reference");
    assert!(error.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("content/main.scene (e_instance.prefab)")
    }));
    assert_eq!(store.revision(), 0);
    let error = registry
        .apply(
            &mut store,
            &batch(
                "asset.move",
                json!({"source":"content/props/box.prefab","destination":"content/main.scene"}),
                false,
            ),
            &Session::human(),
        )
        .expect_err("collision");
    assert!(error.iter().any(|diagnostic| {
        diagnostic.code == ara_diag::codes::INVALID_PARAMS
            && diagnostic.location.command == Some(0)
            && diagnostic.location.path.as_deref() == Some("destination")
            && diagnostic
                .message
                .contains("collides with loaded document `content/main.scene`")
    }));
    assert_eq!(store.revision(), 0);
    assert!(store.workspace().find("content/props/box.prefab").is_some());
}

fn modern() -> (Registry, Store) {
    let mut db = ara_schema::SchemaDb::new();
    ara_doc::register(&mut db);
    let registry = Registry::builtin(&mut db);
    let mut workspace = Workspace::default();
    workspace.set_reference_root("game");
    workspace.insert(
        "ara.project.json",
        document(
            "project",
            "pj_project",
            json!({"p_settings":{"settings":{"name":"props","main":"props/start.scene"}}}),
        ),
    );
    workspace.insert(
        "game/props/box.prefab",
        document(
            "prefab",
            "pf_box",
            json!({"e_root":{"name":"props","order":"a0"}}),
        ),
    );
    workspace.insert(
        "game/props/start.scene",
        document(
            "scene",
            "sc_start",
            json!({"e_use":{"name":"props","prefab":"@props/box.prefab#pf_box","order":"a0"}}),
        ),
    );
    workspace.insert(
        "game/other.scene",
        document(
            "scene",
            "sc_other",
            json!({"e_label":{"name":"props","order":"a0"}}),
        ),
    );
    for (path, state) in [
        ("game/props", FileState::directory()),
        ("game/props/box.prefab", FileState::bytes(b"a")),
        ("game/props/start.scene", FileState::bytes(b"b")),
    ] {
        workspace.observe_file(path.into(), Some(state));
    }
    (registry, Store::with_ids(db, workspace, IdGen::seeded(814)))
}

#[test]
fn authored_relative_duplicate_creates_one_storage_prefix_and_new_ids() {
    let (registry, mut store) = modern();
    let result = registry
        .apply(
            &mut store,
            &batch(
                "asset.duplicate",
                json!({"source":"props/box.prefab","destination":"props/copy.prefab"}),
                false,
            ),
            &Session::human(),
        )
        .expect("duplicate short path");
    let id = store
        .workspace()
        .find("props/copy.prefab")
        .expect("short index path");
    assert_ne!(id.as_str(), "pf_box");
    assert_eq!(
        store.workspace().path_of(id.as_str()),
        Some("game/props/copy.prefab")
    );
    assert!(
        store
            .workspace()
            .docs()
            .all(|(path, _)| !path.starts_with("game/game/"))
    );
    assert_eq!(result.outputs[0]["source"], "props/box.prefab");
    assert_eq!(result.outputs[0]["assets"][0]["path"], "props/copy.prefab");
    assert_eq!(
        store
            .workspace()
            .doc(id.as_str())
            .expect("copy")
            .records()
            .values()
            .next()
            .expect("copied entity")["name"],
        "props"
    );
}

#[test]
fn moving_props_repairs_short_references_and_marker_without_changing_names() {
    let (registry, mut store) = modern();
    registry
        .apply(
            &mut store,
            &batch(
                "asset.move",
                json!({"source":"props","destination":"warehouse"}),
                false,
            ),
            &Session::human(),
        )
        .expect("move short folder");
    assert_eq!(
        store.workspace().path_of("pf_box"),
        Some("game/warehouse/box.prefab")
    );
    assert_eq!(
        store
            .workspace()
            .doc("sc_start")
            .expect("scene")
            .record("e_use")
            .expect("instance")["prefab"],
        "@warehouse/box.prefab#pf_box"
    );
    assert_eq!(
        store
            .workspace()
            .doc("pj_project")
            .expect("marker")
            .record("p_settings")
            .expect("settings")["settings"]["main"],
        "warehouse/start.scene"
    );
    assert_eq!(
        store
            .workspace()
            .doc("pj_project")
            .expect("marker")
            .record("p_settings")
            .expect("settings")["settings"]["name"],
        "props"
    );
    for (doc, record) in [
        ("pf_box", "e_root"),
        ("sc_start", "e_use"),
        ("sc_other", "e_label"),
    ] {
        assert_eq!(
            store
                .workspace()
                .doc(doc)
                .expect("document")
                .record(record)
                .expect("entity")["name"],
            "props"
        );
    }
    store
        .undo_last(&Author::Human)
        .expect("one undo restores paths and references");
    assert_eq!(
        store.workspace().path_of("pf_box"),
        Some("game/props/box.prefab")
    );
    assert_eq!(
        store
            .workspace()
            .doc("sc_start")
            .expect("scene")
            .record("e_use")
            .expect("instance")["prefab"],
        "@props/box.prefab#pf_box"
    );
}

#[test]
fn folder_move_rename_duplicate_and_delete_share_one_transaction_and_undo() {
    let (registry, mut store) = modern();
    let before_files = file_states(&store);
    let mut request = Batch::new("pj_project", []);
    for (command, params) in [
        ("folder.create", json!({"path":"workshop"})),
        (
            "asset.move",
            json!({"source":"props","destination":"workshop/props"}),
        ),
        (
            "asset.rename",
            json!({"source":"workshop/props","name":"crates"}),
        ),
        (
            "asset.duplicate",
            json!({"source":"workshop/crates","destination":"workshop/copy"}),
        ),
        ("asset.delete", json!({"source":"workshop/copy"})),
    ] {
        request.calls.push(Call {
            command: command.into(),
            params,
        });
    }
    let result = registry
        .apply(&mut store, &request, &Session::human())
        .expect("atomic batch");
    assert_eq!(result.outputs.len(), 5);
    assert_eq!(store.history().len(), 1);
    assert_eq!(
        store.workspace().path_of("pf_box"),
        Some("game/workshop/crates/box.prefab")
    );
    assert_eq!(
        store
            .workspace()
            .doc("sc_start")
            .expect("scene")
            .record("e_use")
            .expect("use")["prefab"],
        "@workshop/crates/box.prefab#pf_box"
    );
    assert_eq!(
        store
            .workspace()
            .doc("pj_project")
            .expect("marker")
            .record("p_settings")
            .expect("settings")["settings"]["main"],
        "workshop/crates/start.scene"
    );
    assert!(store.workspace().file_state("game/workshop/copy").is_none());
    assert!(
        store
            .workspace()
            .docs()
            .all(|(path, _)| !path.starts_with("game/workshop/copy/"))
    );
    let after_files = file_states(&store);
    store.undo_last(&Author::Human).expect("one undo");
    assert_eq!(file_states(&store), before_files);
    assert_eq!(
        store.workspace().path_of("pf_box"),
        Some("game/props/box.prefab")
    );
    assert_eq!(
        store
            .workspace()
            .doc("sc_start")
            .expect("scene")
            .record("e_use")
            .expect("use")["prefab"],
        "@props/box.prefab#pf_box"
    );
    store.redo_last(&Author::Human).expect("one redo");
    assert_eq!(file_states(&store), after_files);
    assert_eq!(
        store.workspace().path_of("pf_box"),
        Some("game/workshop/crates/box.prefab")
    );
}

#[test]
fn independent_folder_conflicts_keep_original_command_indices_without_partial_apply() {
    let (registry, mut store) = modern();
    let before = file_states(&store);
    let request = Batch::new(
        "pj_project",
        [
            ("folder.create", json!({"path":"temporary"})),
            ("folder.create", json!({"path":"props"})),
            ("folder.create", json!({"path":"missing/child"})),
        ],
    );
    let errors = registry
        .apply(&mut store, &request, &Session::human())
        .expect_err("both conflicts");
    assert!(
        errors.iter().any(|error| error.location.command == Some(1)
            && error.location.path.as_deref() == Some("path"))
    );
    assert!(errors.iter().any(|error| error.location.command == Some(2)
        && error.location.path.as_deref() == Some("parents")));
    assert_eq!(store.revision(), 0);
    assert!(store.history().is_empty());
    assert_eq!(file_states(&store), before);
    assert!(store.workspace().file_state("game/temporary").is_none());
}

fn file_states(store: &Store) -> std::collections::BTreeMap<String, FileState> {
    store
        .workspace()
        .file_entries()
        .map(|(path, state)| (path.to_owned(), state.clone()))
        .collect()
}

#[test]
fn source_moves_project_new_companion_consumers_for_later_batch_operations() {
    let (registry, mut store) = modern();
    let model = FileState::bytes(b"model");
    let texture = FileState::bytes(b"texture");
    for (path, state) in [
        ("game/models", FileState::directory()),
        ("game/textures", FileState::directory()),
        ("game/models/model.gltf", model.clone()),
        ("game/textures/albedo.png", texture.clone()),
    ] {
        store.observe_file(path.into(), Some(state));
    }
    let mut metadata = AssetMetadata::new("frozen-complete-source-graph".into());
    metadata.inventory.complete = true;
    metadata.inventory.source_paths =
        ["models/model.gltf".into(), "textures/albedo.png".into()].into();
    metadata.inventory.consumers.insert(
        "models/model.gltf".into(),
        AssetConsumer {
            state: model,
            blake3: "a".repeat(64),
            inputs: vec![AssetLocalInput {
                uri: "../textures/albedo.png".into(),
                path: "textures/albedo.png".into(),
                state: texture.clone(),
                blake3: "b".repeat(64),
            }],
        },
    );
    metadata.inventory.consumers.insert(
        "textures/albedo.png".into(),
        AssetConsumer {
            state: texture.clone(),
            blake3: "b".repeat(64),
            inputs: vec![],
        },
    );
    let request = Batch::new(
        "pj_project",
        [
            ("folder.create", json!({"path":"organized"})),
            (
                "asset.move",
                json!({"source":"models/model.gltf","destination":"organized/detail/model.gltf"}),
            ),
            (
                "asset.rename",
                json!({"source":"organized/detail/model.gltf","name":"asset.gltf"}),
            ),
            (
                "asset.duplicate",
                json!({"source":"organized/detail","destination":"organized/copy"}),
            ),
            ("asset.delete", json!({"source":"organized/copy"})),
            ("asset.delete", json!({"source":"textures/albedo.png"})),
        ],
    );
    let before = file_states(&store);
    registry
        .apply_with_metadata(&mut store, &request, &Session::human(), metadata)
        .expect("one projected source graph");
    assert_eq!(store.history().len(), 1);
    assert!(
        store
            .workspace()
            .file_state("game/models/model.gltf")
            .is_none()
    );
    assert!(
        store
            .workspace()
            .file_state("game/organized/detail/asset.gltf")
            .is_some()
    );
    assert_eq!(
        store
            .workspace()
            .file_state("game/organized/textures/albedo.png"),
        Some(&texture)
    );
    assert!(
        store
            .workspace()
            .file_state("game/textures/albedo.png")
            .is_none()
    );
    assert!(
        store
            .workspace()
            .file_state("game/organized/copy")
            .is_none()
    );
    store.undo_last(&Author::Human).expect("one undo");
    assert_eq!(file_states(&store), before);
    store.redo_last(&Author::Human).expect("one redo");
    assert!(
        store
            .workspace()
            .file_state("game/organized/detail/asset.gltf")
            .is_some()
    );
}

#[test]
fn import_preflight_schema_and_deserialization_share_camel_case_fields() {
    let (registry, store) = modern();
    let info = registry.0.info("asset.import").expect("import catalog");
    let schema = store
        .schema()
        .type_json_schema(store.schema().get(&info.params).expect("params type"));
    assert!(schema["properties"].get("dryRun").is_some());
    assert!(schema["properties"].get("dry_run").is_none());
    let preflight = json!({"sources":["C:/Art/props"],"destination":"props","dryRun":true});
    let params: ara_cmd::builtin::AssetImportParams =
        serde_json::from_value(preflight.clone()).expect("camelCase accepted");
    assert!(params.dry_run);
    assert_eq!(
        serde_json::to_value(params).expect("typed params")["dryRun"],
        true
    );
    assert!(
        serde_json::from_value::<ara_cmd::builtin::AssetImportParams>(
            json!({"sources":["C:/Art/props"],"destination":"props","dry_run":true})
        )
        .is_err()
    );
    let mut diagnostics = Vec::new();
    ara_schema::Validator::new(store.schema(), &mut diagnostics).fields(
        store.schema().get(&info.params).expect("params type"),
        preflight.as_object().expect("params object"),
        "params",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    for name in [
        "asset.move",
        "asset.rename",
        "asset.duplicate",
        "asset.delete",
        "asset.import",
        "folder.create",
    ] {
        assert_eq!(
            registry.0.info(name).expect("command registered").scope,
            "edit:assets"
        );
    }
}
