use super::*;
use ara_doc::file_state::FileState;
use serde_json::json;

fn document(kind: &str, id: &str, records: Value) -> Document {
    let mut result = Document::new(kind, AssetId::parse(id).expect("asset ID"));
    for (id, record) in records.as_object().expect("records") {
        result.insert_unchecked(
            RecordId::parse(id).expect("record ID"),
            record.as_object().expect("record").clone(),
        );
    }
    result
}
fn fixture(
    owner: JsonOwner,
    value: Value,
) -> (Workspace, SchemaDb, AssetImportParams, Vec<ImportEntry>) {
    let mut schema = SchemaDb::new();
    ara_doc::register(&mut schema);
    let mut workspace = Workspace::default();
    workspace.set_reference_root("game");
    workspace.insert(
        "game/imported/main.scene",
        document(
            "scene",
            "sc_kept",
            json!({"e_kept":{"name":"Kept","order":"a0"}}),
        ),
    );
    let native = ImportEntry {
        source_reference: "source/main.scene".into(),
        destination: "imported/main.scene".into(),
        disposition: ImportDisposition::Skipped,
        state: FileState::bytes(b"foreign"),
        previous: Some(FileState::bytes(b"kept")),
        existing_id: Some(AssetId::parse("sc_kept").expect("ID")),
        document: Some(document(
            "scene",
            "sc_foreign",
            json!({"e_foreign":{"name":"Foreign","order":"a0"}}),
        )),
        json: None,
    };
    let json = ImportEntry {
        source_reference: "source/bindings.json".into(),
        destination: "imported/bindings.json".into(),
        disposition: ImportDisposition::Created,
        state: FileState::bytes(b"bindings"),
        previous: None,
        existing_id: None,
        document: None,
        json: Some(super::super::super::ImportJson {
            owner,
            context: Some(AssetId::parse("sc_foreign").expect("context")),
            value,
        }),
    };
    (
        workspace,
        schema,
        AssetImportParams {
            sources: vec!["C:/Art/source".into()],
            destination: "imported".into(),
            conflict: super::super::super::ImportConflict::Skip,
            dry_run: false,
        },
        vec![native, json],
    )
}

#[test]
fn skipped_scene_refuses_foreign_ui_records_without_guessing_identity() {
    for (owner, value, field) in [
        (
            JsonOwner::UiBindings,
            json!([{"entity":"e_foreign","component":"Counter","field":"score"}]),
            "0.entity",
        ),
        (
            JsonOwner::UiResources,
            json!({"music":[{"entity":"e_foreign"}]}),
            "music.0.entity",
        ),
        (
            JsonOwner::UiResources,
            json!({"music":[{"target":"e_foreign"}]}),
            "music.0.target",
        ),
    ] {
        let (workspace, schema, request, entries) = fixture(owner, value);
        let error = remap_import(&workspace, &schema, "frozen-skip", &request, &entries)
            .err()
            .expect("foreign entity absent");
        assert!(
            error
                .message
                .contains("absent from skipped native asset `imported/main.scene`")
        );
        assert_eq!(error.location.path.as_deref(), Some(field));
        assert_eq!(
            error.location.file.as_deref(),
            Some("game/imported/bindings.json")
        );
        assert!(workspace.doc("sc_foreign").is_none());
    }
}

#[test]
fn matching_kept_and_nested_prefab_addresses_are_valid_but_bare_names_are_not_references() {
    let (mut workspace, schema, request, mut entries) = fixture(
        JsonOwner::UiBindings,
        json!([{"entity":"e_kept"},{"entity":"e_instance/e_leaf"}]),
    );
    workspace.insert(
        "game/prefabs/child.prefab",
        document(
            "prefab",
            "pf_child",
            json!({"e_childroot":{"name":"Root","order":"a0"},"e_leaf":{"name":"Child","parent":"e_childroot","order":"a0"}}),
        ),
    );
    workspace.insert("game/imported/main.scene",document("scene","sc_kept",json!({"e_kept":{"name":"Kept","order":"a0"},"e_instance":{"name":"Instance","order":"a1","prefab":"@prefabs/child.prefab#pf_child"}})));
    let foreign = entries[0].document.as_mut().expect("foreign scene");
    foreign.insert_unchecked(
        RecordId::parse("e_kept").expect("ID"),
        json!({"name":"e_foreign","order":"a1"})
            .as_object()
            .expect("record")
            .clone(),
    );
    foreign.insert_unchecked(
        RecordId::parse("e_instance").expect("ID"),
        json!({"name":"Instance","order":"a2","prefab":"@prefabs/child.prefab#pf_child"})
            .as_object()
            .expect("record")
            .clone(),
    );
    remap_import(&workspace, &schema, "frozen-skip", &request, &entries)
        .expect("matching retained records and expanded prefab children resolve");
    entries[1].json = Some(super::super::super::ImportJson {
        owner: JsonOwner::Generic,
        context: Some(AssetId::parse("sc_foreign").expect("ID")),
        value: json!({"name":"e_foreign","entity":"e_foreign"}),
    });
    remap_import(&workspace, &schema, "frozen-skip", &request, &entries)
        .expect("generic bare names do not become entity references");
}

#[test]
fn explicit_record_refs_into_skipped_native_assets_require_existing_records() {
    let (workspace, schema, request, mut entries) = fixture(
        JsonOwner::Generic,
        json!({"selection":"@source/main.scene#e_foreign"}),
    );
    let error = remap_import(&workspace, &schema, "frozen-skip", &request, &entries)
        .err()
        .expect("missing explicit record");
    assert_eq!(error.location.path.as_deref(), Some("selection"));
    entries[0].disposition = ImportDisposition::Replaced;
    assert!(
        remap_import(&workspace, &schema, "frozen-skip", &request, &entries).is_ok(),
        "Replace remaps both records and incoming contextual references"
    );
}

#[test]
fn independent_selections_with_ambiguous_source_paths_fail_before_identity_projection() {
    let (workspace, schema, mut request, mut entries) =
        fixture(JsonOwner::Generic, json!({"name":"unchanged"}));
    request.conflict = super::super::super::ImportConflict::Rename;
    entries[0].disposition = ImportDisposition::Created;
    entries[0].existing_id = None;
    entries[0].previous = None;
    entries[0].destination = "imported/first/main.scene".into();
    let mut second = entries[0].clone();
    second.destination = "imported/second/main.scene".into();
    second.document = Some(document(
        "scene",
        "sc_second",
        json!({"e_second":{"name":"Second","order":"a0"}}),
    ));
    entries.push(second);
    let error = remap_import(
        &workspace,
        &schema,
        "multiple-selection-proof",
        &request,
        &entries,
    )
    .err()
    .expect("ambiguous relative source names");
    assert_eq!(error.location.path.as_deref(), Some("sources"));
    assert!(error.message.contains("common containing folder"));
    assert!(workspace.doc("sc_foreign").is_none());
    entries.last_mut().expect("second source").source_reference = "other/main.scene".into();
    assert!(
        remap_import(
            &workspace,
            &schema,
            "multiple-selection-proof",
            &request,
            &entries
        )
        .is_ok()
    );
    for entry in &mut entries {
        entry.document = None;
        entry.json = None;
    }
    entries.last_mut().expect("second source").source_reference = "source/main.scene".into();
    assert!(
        remap_import(
            &workspace,
            &schema,
            "multiple-selection-proof",
            &request,
            &entries
        )
        .is_ok(),
        "ordinary renamed raw files need no ambiguous path remapping"
    );
}
