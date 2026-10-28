use super::super::{
    inventory::Inventory,
    tests::{begin, bindings, call, complete, open, scratch, write_project},
};
use super::*;
use ara_asset::Cancellation;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn write_json(path: &Path, value: &Value) {
    fs::write(
        path,
        ara_json::to_canonical_string(&ara_json::sort_keys(value)),
    )
    .expect("fixture JSON");
}

#[test]
fn conventional_scene_context_keeps_legacy_precedence_and_accepts_scoped_starter() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut bindings = bindings(scratch.path());
    bindings.workspace = ara_doc::Workspace::default();
    bindings.workspace.insert(
        "scenes/main.scene",
        ara_doc::Document::new(
            "scene",
            ara_doc::AssetId::parse("sc_scoped").expect("scoped scene ID"),
        ),
    );
    assert_eq!(
        scene_context(&bindings).expect("scoped starter context"),
        Some(ara_doc::AssetId::parse("sc_scoped").expect("scoped scene ID"))
    );
    bindings.workspace.insert(
        "levels/main.scene",
        ara_doc::Document::new(
            "scene",
            ara_doc::AssetId::parse("sc_legacy").expect("legacy scene ID"),
        ),
    );
    assert_eq!(
        scene_context(&bindings).expect("legacy starter context"),
        Some(ara_doc::AssetId::parse("sc_legacy").expect("legacy scene ID"))
    );
}

#[test]
fn policy_references_resolve_by_id_path_fallback_and_policy_precedence() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut bindings = bindings(scratch.path());
    bindings.workspace.insert(
        bindings.workspace.storage_path("alternate.scene"),
        ara_doc::Document::new(
            "scene",
            ara_doc::AssetId::parse("sc_alternate").expect("alternate ID"),
        ),
    );
    let policy = ara_host::project::read_document(
        r#"{"main":"@stale.scene#sc_alternate","ui":{"resources":"@feature/media.json#as_missing"}}"#,
        ara_host::project_policy::PREFERENCES,
        &bindings.schema,
        None,
    )
    .doc
    .expect("policy document");
    bindings
        .workspace
        .insert_project_document(ara_host::project_policy::PREFERENCES, policy);
    assert_eq!(
        json_owner(
            &bindings.workspace.storage_path("feature/media.json"),
            &bindings
        )
        .expect("path fallback"),
        Some(JsonOwner::UiResources)
    );
    assert_eq!(
        scene_context(&bindings)
            .expect("main ID takes precedence")
            .expect("scene")
            .as_str(),
        "sc_alternate"
    );
    let asset = ara_doc::Document::new(
        "scene",
        ara_doc::AssetId::parse("sc_media").expect("fixture ID"),
    );
    bindings.workspace.insert_with_asset_path(
        bindings.workspace.storage_path("indexed.scene"),
        asset,
        "feature/media.json",
    );
    for reference in ["@#sc_media", "sc_media", "@wrong.json#sc_media"] {
        let policy = ara_host::project::read_document(
            &format!(r#"{{"ui":{{"resources":"{reference}"}}}}"#),
            ara_host::project_policy::PREFERENCES,
            &bindings.schema,
            None,
        )
        .doc
        .expect("policy");
        bindings
            .workspace
            .insert_project_document(ara_host::project_policy::PREFERENCES, policy);
        assert_eq!(
            json_owner(
                &bindings.workspace.storage_path("feature/media.json"),
                &bindings
            )
            .expect("indexed owner"),
            Some(JsonOwner::UiResources)
        );
    }
    let policy = ara_host::project::read_document(
        r#"{"ui":{"resources":"@#as_missing"}}"#,
        ara_host::project_policy::PREFERENCES,
        &bindings.schema,
        None,
    )
    .doc
    .expect("policy");
    bindings
        .workspace
        .insert_project_document(ara_host::project_policy::PREFERENCES, policy);
    let errors = json_owner(
        &bindings.workspace.storage_path("feature/media.json"),
        &bindings,
    )
    .expect_err("unresolvable ID cannot silently lose resource owner");
    assert!(
        errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("ui.resources")
                && diagnostic.message.contains("valid @path#id"))
    );
}

#[test]
fn configured_resources_and_unknown_json_references_follow_move_and_block_delete() {
    let scratch = scratch();
    write_project(scratch.path());
    let root = scratch.path();
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    fs::create_dir_all(root.join("settings")).expect("policy root");
    fs::create_dir_all(authored.join("feature/data")).expect("authored files");
    for file in ["font.bin", "icon.bin", "click.bin"] {
        fs::write(authored.join("feature/data").join(file), file.as_bytes()).expect("raw fixture");
    }
    fs::write(
        authored.join("feature/active.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_feature","records":{}}"#,
    )
    .expect("configured main scene");
    write_json(
        &root.join(ara_host::project_policy::PREFERENCES),
        &json!({"main":"@stale.scene#sc_feature","ui":{"resources":"@feature/media.json#as_missing"}}),
    );
    write_json(
        &authored.join("feature/media.json"),
        &json!({"fonts":["feature/data/font.bin"],"images":{"icon":"feature/data/icon.bin"},"click":"feature/data/click.bin"}),
    );
    write_json(
        &authored.join("feature/extra.json"),
        &json!({"link":"@feature/data/font.bin","name":"feature/data/font.bin"}),
    );
    write_json(
        &authored.join("feature/defaults.types.json"),
        &json!({"$schema":"ara://schema/types/1","types":{"FeatureAssetDefaultFixture":{"kind":"component","doc":"Fixture authoring defaults.","fields":{"resource":{"type":"asset","doc":"Fixture resource.","default":"@feature/data/font.bin"}}}}}),
    );
    let bindings = bindings(root);
    let inventory = Inventory::read(root, &bindings, true, &Cancellation::default())
        .expect("frozen readonly graph");
    let metadata = base(
        &inventory,
        &bindings,
        &crate::DependencyInventory::default(),
        "configured-json-fixture".into(),
    )
    .expect("JSON enrollment");
    assert!(
        metadata
            .json_files
            .iter()
            .any(|file| file.path.ends_with("defaults.types.json")
                && file.owner == JsonOwner::Generic)
    );
    let resources = metadata
        .json_files
        .iter()
        .find(|file| file.owner == JsonOwner::UiResources)
        .expect("configured resources");
    assert_eq!(
        resources.context.as_ref().map(ara_doc::AssetId::as_str),
        Some("sc_feature")
    );
    assert!(!root.join(".ara").exists());
    let mut host = open(root);
    let session = begin(&mut host, None);
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"commands":[{"cmd":"asset.move","args":{"source":"feature/data","destination":"shared"}}]}),
    );
    complete(
        &mut host,
        &session,
        started["job"].as_str().expect("move job"),
    );
    let resources = ara_json::parse(
        &fs::read_to_string(authored.join("feature/media.json")).expect("saved resources"),
    )
    .expect("resources");
    assert_eq!(resources["fonts"][0], "shared/font.bin");
    assert_eq!(resources["images"]["icon"], "shared/icon.bin");
    assert_eq!(resources["click"], "shared/click.bin");
    let extra = ara_json::parse(
        &fs::read_to_string(authored.join("feature/extra.json")).expect("saved generic JSON"),
    )
    .expect("generic JSON");
    assert_eq!(extra["link"], "@shared/font.bin");
    assert_eq!(extra["name"], "feature/data/font.bin");
    let defaults = ara_json::parse(
        &fs::read_to_string(authored.join("feature/defaults.types.json")).expect("saved defaults"),
    )
    .expect("defaults");
    assert_eq!(
        defaults["types"]["FeatureAssetDefaultFixture"]["fields"]["resource"]["default"],
        "@shared/font.bin"
    );
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"commands":[{"cmd":"asset.delete","args":{"source":"shared"}}]}),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.tick();
        let status = call(
            &mut host,
            "jobs.status",
            json!({"session":session,"job":started["job"]}),
        );
        if status["status"] == "failed" {
            let text = status["diagnostics"].to_string();
            assert!(
                text.contains("feature/media.json")
                    && text.contains("feature/extra.json")
                    && text.contains("defaults.types.json"),
                "{text}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "delete preflight did not report references: {status}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(authored.join("shared/font.bin").exists());
}
