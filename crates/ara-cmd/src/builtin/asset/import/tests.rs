use super::super::metadata::AssetMetadata;
use super::*;
use crate::{Batch, Registry as KernelRegistry, Session};
use ara_doc::file_state::PreparedInventory;
use ara_doc::{IdGen, RecordId, Workspace};
use ara_ops::{Author, Store};

fn document(kind: &str, id: &str, records: Value) -> Document {
    let mut doc = Document::new(kind, AssetId::parse(id).expect("asset ID"));
    for (id, value) in records.as_object().expect("records") {
        doc.insert_unchecked(
            RecordId::parse(id).expect("record ID"),
            value.as_object().expect("record").clone(),
        );
    }
    doc
}
fn blob(byte: char) -> FileState {
    FileState::bytes(&[byte as u8])
}
fn entry(source: &str, destination: &str, document: Option<Document>) -> ImportEntry {
    ImportEntry {
        source_reference: source.into(),
        destination: destination.into(),
        disposition: ImportDisposition::Created,
        state: blob('b'),
        previous: None,
        existing_id: None,
        document,
        json: None,
    }
}
fn prepared(request: AssetImportParams) -> PreparedAssetImport {
    let destination = request.destination.as_str();
    let mut prefab = entry(
        "props/box.prefab",
        &format!("{destination}/box.prefab"),
        Some(document(
            "prefab",
            "pf_foreign",
            json!({"e_root":{"name":"props","order":"a0"},"e_lid":{"name":"Lid","parent":"e_root","order":"a0"}}),
        )),
    );
    if request.conflict == ImportConflict::Replace {
        prefab.disposition = ImportDisposition::Replaced;
        prefab.previous = Some(blob('a'));
        prefab.existing_id = Some(AssetId::parse("pf_existing").expect("ID"));
    }
    let scene = entry(
        "props/main.scene",
        &format!("{destination}/main.scene"),
        Some(document(
            "scene",
            "sc_foreign",
            json!({"e_instance":{"name":"props","prefab":"@props/box.prefab#pf_foreign","order":"a0"}}),
        )),
    );
    let image = entry("props/icon.bin", &format!("{destination}/icon.bin"), None);
    let mut resources = entry(
        "props/resources.json",
        &format!("{destination}/resources.json"),
        None,
    );
    resources.json = Some(ImportJson {
        owner: references::JsonOwner::UiResources,
        context: Some(AssetId::parse("sc_foreign").expect("scene ID")),
        value: json!({"fonts":["props/icon.bin"],"images":{"props":"props/icon.bin"},"music":[{"entity":"e_instance","target":"e_instance"}]}),
    });
    let mut bindings = entry(
        "props/bindings.json",
        &format!("{destination}/bindings.json"),
        None,
    );
    bindings.json = Some(ImportJson {
        owner: references::JsonOwner::UiBindings,
        context: Some(AssetId::parse("sc_foreign").expect("scene ID")),
        value: json!([{"entity":"e_instance","component":"Counter","field":"score"}]),
    });
    PreparedAssetImport {
        request,
        token: "complete-state-and-policy-v1".into(),
        entries: vec![prefab, scene, image, resources, bindings],
        after_inventory: super::super::metadata::AssetInventory {
            complete: true,
            ..Default::default()
        },
    }
}

struct Registry(KernelRegistry);
impl Registry {
    fn apply(
        &self,
        store: &mut Store,
        batch: &Batch,
        session: &Session,
    ) -> Result<crate::BatchResult, Vec<Diagnostic>> {
        store.clear_prepared_files();
        let request: AssetImportParams =
            serde_json::from_value(batch.calls[0].params.clone()).expect("typed import request");
        let mut metadata = AssetMetadata::new("complete-state-and-policy-v1".into());
        metadata.inventory.complete = true;
        metadata.import = Some(prepared(request));
        let mut inventory = PreparedInventory {
            entries: store
                .workspace()
                .file_entries()
                .map(|(path, state)| (path.to_owned(), Some(state.clone())))
                .collect(),
            metadata: serde_json::to_value(metadata).expect("metadata"),
            ..PreparedInventory::default()
        };
        let preview =
            super::super::preview_assets(store.workspace(), store.schema(), batch, &mut inventory)?;
        let mut metadata: AssetMetadata =
            serde_json::from_value(inventory.metadata.clone()).expect("projected metadata");
        let scene = preview
            .documents
            .iter()
            .find(|document| document.document.kind == "scene")
            .expect("imported scene");
        let instance = scene
            .document
            .records()
            .keys()
            .next()
            .expect("instance")
            .to_string();
        for blob in &preview.blobs {
            let value = ara_json::parse(std::str::from_utf8(&blob.bytes).expect("canonical JSON"))
                .expect("checked JSON");
            if blob.path.ends_with("bindings.json") {
                assert_eq!(value[0]["entity"], instance);
            }
            if blob.path.ends_with("resources.json") {
                assert_eq!(value["fonts"][0], "imported/icon.bin");
                assert_eq!(value["images"]["props"], "imported/icon.bin");
                assert_eq!(value["music"][0]["entity"], instance);
                assert_eq!(value["music"][0]["target"], instance);
            }
        }
        for document in preview.documents {
            let state =
                FileState::bytes(ara_doc::write(&document.document, store.schema()).as_bytes());
            metadata
                .operations
                .get_mut(&document.operation_key)
                .expect("operation")
                .document_states
                .insert(document.path, state);
        }
        inventory.metadata = serde_json::to_value(metadata).expect("prepared metadata");
        super::super::preview_assets(store.workspace(), store.schema(), batch, &mut inventory)?;
        store
            .prepare_files(inventory)
            .map_err(|diagnostic| vec![diagnostic])?;
        let result = self.0.apply(store, batch, session);
        store.clear_prepared_files();
        result
    }
}
fn setup(replace: bool) -> (Registry, Store) {
    let mut db = ara_schema::SchemaDb::new();
    ara_doc::register(&mut db);
    let registry = Registry(KernelRegistry::builtin(&mut db));
    let mut workspace = Workspace::default();
    workspace.set_reference_root("game");
    workspace.insert(
        "ara.project.json",
        document(
            "project",
            "pj_project",
            json!({"p_settings":{"settings":{"name":"Import test"}}}),
        ),
    );
    if replace {
        workspace.insert(
            "game/imported/box.prefab",
            document(
                "prefab",
                "pf_existing",
                json!({"e_root":{"name":"Old","order":"a0"}}),
            ),
        );
        workspace.observe_file("game/imported/box.prefab".into(), Some(blob('a')));
    }
    (registry, Store::with_ids(db, workspace, IdGen::seeded(835)))
}
fn batch(conflict: &str) -> Batch {
    Batch::new(
        "pj_project",
        [(
            "asset.import",
            json!({"sources":["C:/Art/props"],"destination":"imported","conflict":conflict}),
        )],
    )
}
#[test]
fn import_remaps_foreign_asset_entity_ui_ids_and_all_changes_share_one_undo() {
    let (registry, mut store) = setup(false);
    let result = registry
        .apply(&mut store, &batch("rename"), &Session::human())
        .expect("prepared commit");
    assert_eq!(store.history().len(), 1);
    let prefab = store
        .workspace()
        .find("imported/box.prefab")
        .expect("new prefab")
        .clone();
    let scene = store
        .workspace()
        .find("imported/main.scene")
        .expect("new scene")
        .clone();
    assert_ne!(prefab.as_str(), "pf_foreign");
    assert_ne!(scene.as_str(), "sc_foreign");
    let imported = store.workspace().doc(prefab.as_str()).expect("prefab");
    assert!(
        imported
            .records()
            .keys()
            .all(|id| !matches!(id.as_str(), "e_root" | "e_lid"))
    );
    assert_eq!(
        imported
            .records()
            .values()
            .find(|record| record["name"] == "props")
            .expect("root")["name"],
        "props"
    );
    let instance = store
        .workspace()
        .doc(scene.as_str())
        .expect("scene")
        .records()
        .values()
        .next()
        .expect("instance");
    assert_eq!(instance["prefab"], format!("@imported/box.prefab#{prefab}"));
    assert!(
        result.outputs[0]["created"]
            .as_array()
            .expect("created")
            .iter()
            .any(|entry| entry["path"] == "imported/box.prefab"
                && entry["asset"] == prefab.to_string())
    );
    assert!(
        store
            .workspace()
            .file_entries()
            .any(|(path, _)| path == "game/imported/icon.bin")
    );
    store.undo_last(&Author::Human).expect("one undo");
    assert!(store.workspace().find("imported/box.prefab").is_none());
    assert!(
        !store
            .workspace()
            .file_entries()
            .any(|(path, _)| path.starts_with("game/imported/"))
    );
    store
        .redo_last(&Author::Human)
        .expect("redo same immutable import");
    assert_eq!(store.workspace().find("imported/box.prefab"), Some(&prefab));
}

#[test]
fn replace_retains_destination_identity_matching_records_and_restores_old_bytes_on_undo() {
    let (registry, mut store) = setup(true);
    let original = store
        .workspace()
        .doc("pf_existing")
        .expect("old asset")
        .clone();
    registry
        .apply(&mut store, &batch("replace"), &Session::human())
        .expect("replace");
    assert_eq!(
        store
            .workspace()
            .find("imported/box.prefab")
            .map(AssetId::as_str),
        Some("pf_existing")
    );
    assert_eq!(
        store
            .workspace()
            .doc("pf_existing")
            .expect("replaced")
            .record("e_root")
            .expect("identity retained")["name"],
        "props"
    );
    store.undo_last(&Author::Human).expect("undo replacement");
    assert_eq!(store.workspace().doc("pf_existing"), Some(&original));
    assert_eq!(
        store
            .workspace()
            .file_entries()
            .find(|(path, _)| *path == "game/imported/box.prefab")
            .map(|(_, state)| state),
        Some(&blob('a'))
    );
}

#[test]
fn public_asset_import_refuses_unprepared_direct_execution_without_io() {
    let (registry, mut store) = setup(false);
    let batch = Batch::new(
        "pj_project",
        [(
            "asset.import",
            json!({"sources":["C:/Art/props"],"destination":"imported"}),
        )],
    );
    let error = registry
        .0
        .apply(&mut store, &batch, &Session::human())
        .expect_err("host must prepare async job");
    assert!(
        error
            .iter()
            .any(|diagnostic| diagnostic.message.contains("prepar"))
    );
    assert_eq!(store.revision(), 0);
    assert!(store.workspace().find("imported/box.prefab").is_none());
}

#[test]
fn readonly_import_preflight_returns_exact_conflict_fields_without_ops_or_revision() {
    let (registry, mut store) = setup(true);
    let args = json!({"sources":["C:/Art/props"],"destination":"imported","dryRun":true});
    let request: AssetImportParams = serde_json::from_value(args.clone()).expect("typed dry run");
    let token = "checked-readonly-preflight".to_owned();
    let mut metadata = AssetMetadata::new(token.clone());
    metadata.preflight = Some(PreparedImportPreflight {
        request,
        token,
        conflicts: vec![
            ImportConflictItem {
                source: "C:/Art/props/box.prefab".into(),
                destination_logical_path: "imported/box.prefab".into(),
                existing_asset_id: Some(AssetId::parse("pf_existing").expect("existing ID")),
                kind: ImportConflictKind::File,
            },
            ImportConflictItem {
                source: "C:/Art/props/empty".into(),
                destination_logical_path: "imported/empty".into(),
                existing_asset_id: None,
                kind: ImportConflictKind::Directory,
            },
        ],
        planned: vec![ImportPlannedItem {
            source: Some("C:/Art/props/box.prefab".into()),
            destination_logical_path: "imported/box 2.prefab".into(),
            existing_asset_id: None,
            kind: ImportEntryKind::File,
            action: ImportDisposition::Renamed,
        }],
    });
    let entries = store
        .workspace()
        .file_entries()
        .map(|(path, state)| (path.to_owned(), Some(state.clone())))
        .collect();
    let before = store.workspace().clone();
    store
        .prepare_files(PreparedInventory {
            entries,
            metadata: serde_json::to_value(metadata).expect("metadata"),
            ..Default::default()
        })
        .expect("frozen preflight");
    let result = registry
        .0
        .apply(
            &mut store,
            &Batch::new("pj_project", [("asset.import", args)]),
            &Session::human(),
        )
        .expect("readonly output");
    store.clear_prepared_files();
    assert!(result.ops.is_empty());
    assert!(result.txn.is_none());
    assert_eq!(store.revision(), 0);
    assert!(store.history().is_empty());
    assert_eq!(
        store.workspace().doc("pf_existing"),
        before.doc("pf_existing")
    );
    assert_eq!(
        store.workspace().file_state("game/imported/box.prefab"),
        before.file_state("game/imported/box.prefab")
    );
    assert_eq!(
        result.outputs[0]["conflicts"][0],
        json!({"source":"C:/Art/props/box.prefab","destinationLogicalPath":"imported/box.prefab","existingAssetId":"pf_existing","kind":"file"})
    );
    assert!(result.outputs[0]["conflicts"][1]["existingAssetId"].is_null());
    assert_eq!(result.outputs[0]["planned"][0]["action"], "renamed");
    assert!(result.outputs[0].get("dry_run").is_none());
}

#[test]
fn mismatched_preflight_inventory_token_is_rejected_without_transaction() {
    let (registry, mut store) = setup(false);
    let args = json!({"sources":["C:/Art/props"],"destination":"imported","dryRun":true});
    let mut metadata = AssetMetadata::new("current-inventory".into());
    metadata.preflight = Some(PreparedImportPreflight {
        request: serde_json::from_value(args.clone()).expect("typed dry run"),
        token: "unrelated-old-inventory".into(),
        conflicts: vec![],
        planned: vec![],
    });
    store
        .prepare_files(PreparedInventory {
            metadata: serde_json::to_value(metadata).expect("metadata"),
            ..Default::default()
        })
        .expect("prepared facts");
    let errors = registry
        .0
        .apply(
            &mut store,
            &Batch::new("pj_project", [("asset.import", args)]),
            &Session::human(),
        )
        .expect_err("wrong inventory");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("different inventory"))
    );
    assert_eq!(store.revision(), 0);
    assert!(store.history().is_empty());
}
