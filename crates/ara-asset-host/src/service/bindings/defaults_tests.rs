//! Effective cook inputs must not become explicit sidecar overrides.
use super::*;

fn bindings() -> Bindings {
    let root = crate::service::tests::scratch();
    crate::service::tests::write_project(root.path());
    let mut bindings = crate::service::tests::bindings(root.path());
    ara_asset::register_schema(&mut bindings.schema);
    ara_importers::registry()
        .expect("installed registry")
        .register_schemas(&mut bindings.schema);
    bindings
        .versions
        .insert("ara.texture".into(), "fixture".into());
    bindings.policy.defaults.texture = Some(ara_asset::TextureSettings {
        max_dimension: 512,
        ..Default::default()
    });
    bindings
}
fn raw() -> ImportSettings {
    ImportSettings {
        importer: "ara.texture".into(),
        version: "fixture".into(),
        settings: json!({}),
        ..Default::default()
    }
}
#[test]
fn authoritative_policy_cooks_effectively_but_new_sources_remain_inherited() {
    let bindings = bindings();
    let source = bindings
        .template("tex.png", "as_defaults".into(), &raw())
        .expect("binding");
    assert_eq!(source.settings["maxDimension"], 512);
    let document = crate::service::import_entries::sidecar_document(&source, None, &bindings)
        .expect("new sidecar");
    let record = document.record("i_settings").expect("settings");
    assert_eq!(record["settings"], json!({}));
    assert!(!record.contains_key("preset"));
}
#[test]
fn foreign_and_existing_explicit_overrides_are_preserved_independently() {
    let mut bindings = bindings();
    let mut original = raw();
    original.settings = json!({"maxDimension":2048,"mipmaps":false});
    let mut document =
        ara_doc::Document::new("import", ara_doc::AssetId::parse("as_foreign").expect("ID"));
    document
        .apply(
            ara_asset::SidecarCommand::InitializeInherited(original.clone())
                .edit(document.id.clone())
                .expect("edit"),
            &bindings.schema,
        )
        .expect("fixture");
    let text = ara_doc::write(&document, &bindings.schema);
    let source = bindings
        .template("tex.png", "as_defaults".into(), &original)
        .expect("binding");
    let preserved =
        crate::service::import_entries::sidecar_document(&source, Some(text.as_bytes()), &bindings)
            .expect("foreign");
    assert_eq!(ara_doc::write(&preserved, &bindings.schema), text);
    let storage = bindings.workspace.storage_path("tex.png.import.json");
    bindings.workspace.insert(storage, document);
    let source = bindings
        .existing("tex.png")
        .expect("existing read")
        .expect("source");
    assert_eq!(source.settings["maxDimension"], 2048);
    assert_eq!(source.settings["mipmaps"], false);
    assert_eq!(
        bindings
            .workspace
            .doc("as_foreign")
            .expect("unchanged sidecar")
            .record("i_settings")
            .expect("settings")["settings"],
        original.settings
    );
}
