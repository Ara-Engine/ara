#![allow(clippy::unwrap_used)]
use ara_asset::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn input() -> ImportInput {
    ImportInput {
        preset: Default::default(),
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
    assert!(cache.load(key).is_err());
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
        preset: Default::default(),
        importer: "ara.gltf".into(),
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
    assert_eq!(sidecar.settings().unwrap().importer, "ara.gltf");
    assert_eq!(sidecar.settings().unwrap().preset, CookPreset::HighQuality);
    let desc = store.schema().get("ImportSettings").unwrap();
    let field = desc.fields().iter().find(|f| f.name == "preset").unwrap();
    assert_eq!(store.schema().field_default(field), json!("highQuality"));
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
        CookPreset::Balanced
    );
    store.undo_last(&ara_ops::Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc(id.as_str()).unwrap(), store.schema())
    );
}
