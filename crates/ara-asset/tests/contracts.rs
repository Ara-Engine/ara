#![allow(clippy::unwrap_used)]
use ara_asset::*;

#[test]
fn project_quality_defaults_round_trip_and_reject_invalid_settings() {
    let root = tempfile::tempdir().unwrap();
    let mut db = ara_schema::SchemaDb::new();
    register_schema(&mut db);
    assert_eq!(
        ProjectImportSettings::read(root.path(), &db)
            .unwrap()
            .preset,
        CookPreset::Balanced
    );
    let settings = ProjectImportSettings {
        preset: CookPreset::HighQuality,
    };
    let text = settings.canonical_text().unwrap();
    let path = root.path().join(PROJECT_IMPORT_SETTINGS_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &text).unwrap();
    assert_eq!(
        ProjectImportSettings::read(root.path(), &db).unwrap(),
        settings
    );
    let parsed = ara_doc::read(&text, PROJECT_IMPORT_SETTINGS_FILE, &db, None);
    assert_eq!(text, ara_doc::write(&parsed.doc.unwrap(), &db));
    std::fs::write(&path, text.replace("highQuality", "invalid")).unwrap();
    assert!(ProjectImportSettings::read(root.path(), &db).is_err());
}
#[test]
fn scoped_policy_precedes_legacy_without_rewriting_the_legacy_file() {
    let root = tempfile::tempdir().unwrap();
    let mut db = ara_schema::SchemaDb::new();
    register_schema(&mut db);
    assert_eq!(PROJECT_IMPORT_SETTINGS_FILE, "settings/ara.imports");
    let legacy = ProjectImportSettings {
        preset: CookPreset::Balanced,
    };
    std::fs::write(
        root.path().join(LEGACY_PROJECT_IMPORT_SETTINGS_FILE),
        legacy.canonical_text().unwrap(),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("settings")).unwrap();
    let scoped = ProjectImportSettings {
        preset: CookPreset::HighQuality,
    };
    std::fs::write(
        root.path().join("settings/ara.imports"),
        scoped.canonical_text().unwrap(),
    )
    .unwrap();
    assert_eq!(
        ProjectImportSettings::read(root.path(), &db).unwrap(),
        scoped
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join(LEGACY_PROJECT_IMPORT_SETTINGS_FILE)).unwrap(),
        legacy.canonical_text().unwrap()
    );
    std::fs::remove_file(root.path().join("settings/ara.imports")).unwrap();
    assert_eq!(
        ProjectImportSettings::read(root.path(), &db).unwrap(),
        legacy
    );
}

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn input() -> ImportInput {
    ImportInput {
        preset: CookPreset::HighQuality,
        thread_budget: 16,
        source_path: "test.bin".into(),
        source: b"source".to_vec(),
        importer: "test".into(),
        version: "1".into(),
        settings: json!({"b":2,"a":1}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::from([("dep.bin".into(), vec![4, 5])]),
        asset_dependencies: BTreeMap::new(),
    }
}
fn output() -> ImportOutput {
    ImportOutput {
        artifacts: vec![Artifact {
            name: "custom".into(),
            kind: ArtifactKind::Custom,
            bytes: vec![1, 2, 3],
        }],
    }
}

#[test]
fn action_keys_cover_every_input_and_ignore_json_insertion_order() {
    let base = input();
    let key = base.key().unwrap();
    assert_eq!(base.key_with_backend(None).unwrap(), key);
    let gpu = base.key_with_backend(Some("gpu-fixture-v1")).unwrap();
    assert_ne!(gpu, key, "GPU and CPU artifacts cannot alias");
    assert_ne!(gpu, base.key_with_backend(Some("gpu-fixture-v2")).unwrap());
    let mut reordered = base.clone();
    reordered.settings = json!({"a":1,"b":2});
    assert_eq!(key, reordered.key().unwrap());
    reordered.thread_budget = 1;
    assert_eq!(key, reordered.key().unwrap());
    let mut variants = Vec::new();
    let mut v = base.clone();
    v.preset = CookPreset::Fast;
    variants.push(v);
    let mut v = base.clone();
    v.preset = CookPreset::Balanced;
    variants.push(v);
    let mut v = base.clone();
    v.source.push(0);
    variants.push(v);
    let mut v = base.clone();
    v.source_path = "other.bin".into();
    variants.push(v);
    let mut v = base.clone();
    v.importer = "other".into();
    variants.push(v);
    let mut v = base.clone();
    v.version = "2".into();
    variants.push(v);
    let mut v = base.clone();
    v.settings["a"] = json!(3);
    variants.push(v);
    let mut v = base.clone();
    v.target = TargetPlatform::Linux;
    variants.push(v);
    let mut v = base.clone();
    v.dependencies.get_mut("dep.bin").unwrap().push(0);
    variants.push(v);
    let mut v = base.clone();
    v.asset_dependencies
        .insert("as_a".into(), Digest::of(b"asset"));
    variants.push(v);
    for changed in variants {
        assert_ne!(key, changed.key().unwrap());
    }
}
#[test]
fn cache_is_immutable_verified_and_garbage_collectable() {
    let root = tempfile::tempdir().unwrap();
    let cache = FileCache::new(root.path()).unwrap();
    let key = input().key().unwrap();
    assert!(cache.load(key).unwrap().is_none());
    cache.store(key, &output()).unwrap();
    assert_eq!(cache.load(key).unwrap().unwrap().output, output());
    cache.store(key, &output()).unwrap();
    let mut conflict = output();
    conflict.artifacts[0].bytes.push(4);
    assert!(cache.store(key, &conflict).is_err());
    let old = Digest::of(b"old");
    cache.store(old, &conflict).unwrap();
    let stats = cache.collect(&BTreeSet::from([key])).unwrap();
    assert_eq!(stats.actions_removed, 1);
    assert_eq!(stats.objects_removed, 1);
    assert!(cache.load(old).unwrap().is_none());
    assert_eq!(cache.load(key).unwrap().unwrap().output, output());
    let hash = Digest::of(&output().artifacts[0].bytes);
    std::fs::write(cache.root().join("objects").join(hash.to_string()), b"bad").unwrap();
    assert!(cache.load(key).unwrap().is_none());
    cache.store(key, &output()).unwrap();
    assert_eq!(cache.load(key).unwrap().unwrap().output, output());
}

#[test]
fn cache_torn_actions_and_missing_objects_recook_without_serving_partial_results() {
    let root = tempfile::tempdir().unwrap();
    let cache = FileCache::new(root.path()).unwrap();
    let key = input().key().unwrap();
    let expected = output();
    cache.store(key, &expected).unwrap();
    let action = cache.root().join("actions").join(key.to_string());
    std::fs::write(&action, b"{\"version\":").unwrap();
    assert!(cache.load(key).unwrap().is_none());
    cache.store(key, &expected).unwrap();
    assert_eq!(cache.load(key).unwrap().unwrap().output, expected);
    let object = cache
        .root()
        .join("objects")
        .join(Digest::of(&expected.artifacts[0].bytes).to_string());
    std::fs::remove_file(&object).unwrap();
    assert!(cache.load(key).unwrap().is_none());
    cache.store(key, &expected).unwrap();
    assert_eq!(cache.load(key).unwrap().unwrap().output, expected);
}
#[test]
fn dependency_graph_removes_edges_and_invalidates_transitively_through_cycles() {
    let mut g = DependencyGraph::default();
    g.replace("texture", ["image.png".into()]);
    g.replace("material", ["texture".into()]);
    g.replace("scene", ["material".into()]);
    g.replace("image.png", ["scene".into()]);
    assert_eq!(g.users("texture").collect::<Vec<_>>(), ["material"]);
    assert_eq!(
        g.invalidated_by("image.png"),
        BTreeSet::from(["texture".into(), "material".into(), "scene".into()])
    );
    g.replace("material", ["other".into()]);
    assert!(g.users("texture").next().is_none());
    assert_eq!(
        g.invalidated_by("image.png"),
        BTreeSet::from(["texture".into()])
    );
}
#[test]
fn sidecars_are_kernel_transactions_and_canonical_indexable_documents() {
    let mut db = ara_schema::SchemaDb::new();
    ara_doc::register(&mut db);
    register_schema(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    let id = ara_doc::AssetId::parse("as_fixture").unwrap();
    let mut ws = ara_doc::Workspace::default();
    ws.insert(
        "mesh.glb.import.json",
        Sidecar::empty(id.clone()).into_document(),
    );
    let mut store = ara_ops::Store::new(db, ws);
    let settings = ImportSettings {
        preset: Some(CookPreset::HighQuality),
        importer: "test.gltf".into(),
        version: "1".into(),
        settings: json!({"lodCount":4}),
        dependencies: Vec::new(),
    };
    store
        .transact(
            ara_ops::TxnMeta::by(ara_ops::Author::Human),
            vec![
                SidecarCommand::Initialize(settings)
                    .edit(id.clone())
                    .unwrap(),
            ],
        )
        .unwrap();
    let before = ara_doc::write(store.workspace().doc(id.as_str()).unwrap(), store.schema());
    let sidecar = Sidecar::read(&before, "mesh.glb.import.json", store.schema()).unwrap();
    assert_eq!(before, sidecar.write(store.schema()));
    assert_eq!(sidecar.settings().unwrap().importer, "test.gltf");
    assert_eq!(
        sidecar.settings().unwrap().preset,
        Some(CookPreset::HighQuality)
    );
    let desc = store.schema().get("ImportSettings").unwrap();
    let field = desc.fields().iter().find(|f| f.name == "preset").unwrap();
    assert_eq!(store.schema().field_default(field), serde_json::Value::Null);
    let mut index = ara_doc::AssetIndex::new();
    sidecar.index("models/mesh.glb", &mut index);
    assert_eq!(
        ara_doc::refs::resolve("@old.glb#as_fixture", None, &index)
            .unwrap()
            .0,
        "@models/mesh.glb#as_fixture"
    );
    store
        .transact(
            ara_ops::TxnMeta::by(ara_ops::Author::Human),
            vec![
                SidecarCommand::Settings(json!({"lodCount":2}))
                    .edit(id.clone())
                    .unwrap(),
                SidecarCommand::Preset(CookPreset::Balanced)
                    .edit(id.clone())
                    .unwrap(),
            ],
        )
        .unwrap();
    let changed = ara_doc::write(store.workspace().doc(id.as_str()).unwrap(), store.schema());
    assert_eq!(
        Sidecar::read(&changed, "mesh.glb.import.json", store.schema())
            .unwrap()
            .settings()
            .unwrap()
            .preset,
        Some(CookPreset::Balanced)
    );
    store.undo_last(&ara_ops::Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc(id.as_str()).unwrap(), store.schema())
    );
}

#[test]
fn importer_upgrades_are_recorded_and_only_older_importers_fail() {
    let recorded = |version: &str| ImportSettings {
        importer: "ara.gltf".into(),
        version: version.into(),
        ..Default::default()
    };
    let installed = "3-gltf14-f32text";
    assert_eq!(
        recorded(installed).check_version(installed),
        VersionCheck::Current
    );
    for older in ["2-gltf14", "3-gltf14", "legacy"] {
        assert_eq!(
            recorded(older).check_version(installed),
            VersionCheck::Upgrade,
            "{older}"
        );
    }
    assert_eq!(
        recorded("4-gltf15").check_version(installed),
        VersionCheck::Newer
    );
    let id = ara_doc::AssetId::parse("as_fixture").unwrap();
    match SidecarCommand::Version(installed.into()).edit(id).unwrap() {
        ara_doc::Edit::Set { path, value, .. } => {
            assert_eq!(path.to_string(), "version");
            assert_eq!(value, Some(json!(installed)));
        }
        _ => panic!("expected a set"),
    }
}
