use super::super::{
    import_entries::entry_metadata,
    inventory::Inventory,
    tests::{bindings, scratch, write_project},
};
use super::*;
use std::{fs, path::Path};

fn entry(path: &Path, reference: &str, destination: &str, native: bool) -> import::Entry {
    let bytes = fs::read(path).expect("selected bytes");
    import::Entry {
        source: Some(path.to_owned()),
        source_reference: reference.into(),
        destination: destination.into(),
        kind: import::EntryKind::File,
        disposition: import::Disposition::Created,
        bytes: bytes.len() as u64,
        digest: Some(ara_asset::Digest::of(&bytes)),
        staged: None,
        asset_id: None,
        native_document: native,
    }
}

#[test]
fn same_selection_dependencies_use_post_remap_identity_and_exact_native_bytes_before_cook() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut base = bindings(scratch.path());
    ara_asset::register_schema(&mut base.schema);
    ara_importers::registry()
        .expect("importers")
        .register_schemas(&mut base.schema);
    base.versions
        .insert("ara.texture".into(), "4-current".into());
    let cancellation = Cancellation::default();
    let inventory = Inventory::read(scratch.path(), &base, true, &cancellation)
        .expect("readonly frozen destination");
    base.capture_assets(&inventory)
        .expect("native destination hashes");
    let selected = scratch.path().join("selection");
    fs::create_dir(&selected).expect("selected folder");
    let scene = selected.join("foreign.scene");
    fs::write(
        &scene,
        r#"{"$schema":"ara://schema/scene/1","id":"sc_foreign","records":{}}"#,
    )
    .expect("foreign native asset");
    let texture = selected.join("texture.png");
    fs::write(
        &texture,
        b"selected source bytes; cooking is intentionally excluded from this plan test",
    )
    .expect("source bytes");
    let sidecar = selected.join("texture.png.import.json");
    fs::write(&sidecar,r#"{"$schema":"ara://schema/import/1","id":"as_foreign","records":{"i_settings":{"importer":"ara.texture","version":"3-old","settings":{},"dependencies":["@selection/foreign.scene#sc_foreign"]}}}"#).expect("foreign sidecar");
    let entries = vec![
        entry(
            &scene,
            "selection/foreign.scene",
            "imported/foreign.scene",
            true,
        ),
        entry(
            &texture,
            "selection/texture.png",
            "imported/texture.png",
            false,
        ),
        entry(
            &sidecar,
            "selection/texture.png.import.json",
            "imported/texture.png.import.json",
            true,
        ),
    ];
    let request = AssetImportParams {
        sources: vec![selected.to_string_lossy().into_owned()],
        destination: "imported".into(),
        conflict: ara_cmd::builtin::ImportConflict::Rename,
        dry_run: true,
    };
    let provider = ImportBindings {
        root: scratch.path().to_owned(),
        base: base.clone(),
        inventory: inventory.clone(),
        token: "frozen-native-import-plan".into(),
        request: request.clone(),
    };
    let reader = |entry: &import::Entry| import::read_entry(entry, &cancellation);
    let planned = provider
        .prepare_plan(&entries, &reader, &cancellation)
        .expect("same-selection native dependency plan")
        .expect("immutable source bindings");
    let binding = planned
        .source("imported/texture.png", None)
        .expect("resolved binding")
        .expect("source importer");
    let original = entries
        .iter()
        .map(|entry| {
            entry_metadata(
                scratch.path(),
                &base,
                &inventory,
                entry,
                true,
                &cancellation,
            )
            .expect("native metadata")
        })
        .collect::<Vec<_>>();
    let remap = ara_cmd::builtin::remap_import(
        &base.workspace,
        &base.schema,
        "frozen-native-import-plan",
        &request,
        &original,
    )
    .expect("same pure ID plan");
    let (path, document) = remap
        .documents
        .iter()
        .find(|(path, _)| path == "imported/foreign.scene")
        .expect("remapped native document");
    assert_ne!(document.id.as_str(), "sc_foreign");
    let digest = ara_asset::Digest::of(
        ara_host::project::write_document(document, &base.schema)
            .expect("native bytes")
            .as_bytes(),
    );
    assert_eq!(binding.asset_dependencies.get(path), Some(&digest));
    assert_eq!(
        binding.asset_dependencies.get(document.id.as_str()),
        Some(&digest)
    );
    assert!(!binding.asset_dependencies.contains_key("sc_foreign"));
    assert_eq!(binding.version, "4-current");
    assert!(
        !scratch.path().join(".ara").exists(),
        "readonly native planning created staging/history"
    );
}
