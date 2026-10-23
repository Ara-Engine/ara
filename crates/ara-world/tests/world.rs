#![allow(clippy::unwrap_used)]
use ara_doc::{AssetId, Document, Edit, RecordId, Workspace};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::{FieldPath, SchemaDb};
use ara_world::*;
use serde_json::{Value, json};

fn schema() -> SchemaDb {
    let mut schema = SchemaDb::new();
    register(&mut schema);
    ara_components::register(&mut schema);
    schema
}
fn document(kind: &str, id: &str, rows: &[(&str, Value)]) -> Document {
    let mut doc = Document::new(kind, AssetId::parse(id).unwrap());
    for (id, value) in rows {
        doc.insert_unchecked(
            RecordId::parse(id).unwrap(),
            serde_json::from_value(value.clone()).unwrap(),
        );
    }
    doc
}
fn fixture() -> (Store, AssetId) {
    let world = document(
        "world",
        "wd_test",
        &[
            ("w_grid", json!({"cellSize":250})),
            (
                "l_gameplay",
                json!({"name":"Gameplay","loaded":true,"visible":true,"range":30,"graceTicks":3}),
            ),
            (
                "c_a",
                json!({"x":0,"z":0,"slot":0,"layer":"l_gameplay","documents":["@cells/a.scene#sc_a"]}),
            ),
            (
                "c_b",
                json!({"x":1,"z":0,"slot":0,"layer":"l_gameplay","documents":["@cells/b.scene#sc_b"]}),
            ),
        ],
    );
    let a = document(
        "scene",
        "sc_a",
        &[
            (
                "e_tree",
                json!({"name":"Tree","Transform":{"position":[20,0,30]}}),
            ),
            (
                "e_rock",
                json!({"name":"Rock","Transform":{"position":[40,0,50]}}),
            ),
        ],
    );
    let b = document(
        "scene",
        "sc_b",
        &[(
            "e_door",
            json!({"name":"Door","Transform":{"position":[270,0,10]}}),
        )],
    );
    let id = world.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("outpost.world", world);
    workspace.insert("cells/a.scene", a);
    workspace.insert("cells/b.scene", b);
    (Store::new(schema(), workspace), id)
}

#[test]
fn canonical_roundtrip_spatial_query_includes_unloaded_cells_and_tools_are_typed() {
    let (mut store, id) = fixture();
    let manifest = Manifest::read(store.workspace().doc(id.as_str()).unwrap()).unwrap();
    for (path, doc) in store.workspace().docs() {
        let text = ara_doc::write(doc, store.schema());
        let read = ara_doc::read(&text, path, store.schema(), None);
        assert!(!read.diags.iter().any(|d| d.is_error()), "{:?}", read.diags);
        assert_eq!(text, ara_doc::write(&read.doc.unwrap(), store.schema()));
    }
    let index = TextIndex::build(&manifest, store.workspace(), store.schema()).unwrap();
    assert_eq!(index.len(), 3);
    let tools = WorldTools::new(id, index, store.revision());
    let result = tools
        .call(
            "world.query",
            json!({"min":[260,-1,0],"max":[300,1,30],"layer":null,"limit":10}),
            &mut store,
            TxnMeta::by(Author::Human),
            |_| false,
        )
        .unwrap();
    assert_eq!(result[0]["entity"]["id"], "e_door");
    assert_eq!(result[0]["loaded"], false);
    let mut db = store.schema().clone();
    let catalog = WorldTools::catalog(&mut db);
    assert_eq!(catalog.as_array().unwrap().len(), 4);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
}

#[test]
fn cross_cell_move_is_one_transaction_undoable_and_stale_bases_reject_atomically() {
    let (mut store, id) = fixture();
    let before = store.workspace().clone();
    let command = WorldCommand::MoveToCell(MoveToCell {
        entity: "@stale.scene#e_tree".into(),
        cell: "c_b".into(),
    });
    let mut meta = TxnMeta::by(Author::Agent("builder".into()));
    meta.idempotency_key = Some("move-tree".into());
    let transaction = command
        .apply(&mut store, &id, meta.clone())
        .unwrap()
        .txn()
        .unwrap()
        .clone();
    assert_eq!(transaction.ops.len(), 2);
    assert!(
        store
            .workspace()
            .doc("sc_a")
            .unwrap()
            .record("e_tree")
            .is_none()
    );
    assert!(
        store
            .workspace()
            .doc("sc_b")
            .unwrap()
            .record("e_tree")
            .is_some()
    );
    assert_eq!(
        command
            .apply(&mut store, &id, meta)
            .unwrap()
            .txn()
            .unwrap()
            .id,
        transaction.id
    );
    store.undo(&transaction.id, Author::Human).unwrap();
    for (_, doc) in before.docs() {
        assert_eq!(store.workspace().doc(doc.id.as_str()).unwrap(), doc);
    }
    let base = store.revision();
    store
        .transact(
            TxnMeta::by(Author::Human),
            vec![Edit::Set {
                doc: AssetId::parse("sc_a").unwrap(),
                id: RecordId::parse("e_tree").unwrap(),
                path: FieldPath::field("name"),
                value: Some("Updated".into()),
            }],
        )
        .unwrap();
    let mut stale = TxnMeta::by(Author::Agent("stale".into()));
    stale.base = Some(base);
    assert!(command.apply(&mut store, &id, stale).is_err());
    assert!(
        store
            .workspace()
            .doc("sc_a")
            .unwrap()
            .record("e_tree")
            .is_some()
    );
    assert!(
        store
            .workspace()
            .doc("sc_b")
            .unwrap()
            .record("e_tree")
            .is_none()
    );
}

#[test]
fn standalone_actor_membership_moves_without_changing_its_readable_reference() {
    let (mut store, id) = fixture();
    store.load_doc(
        "actors/e_radio.scene",
        document("scene", "sc_radio", &[("e_radio", json!({"name":"Radio"}))]),
    );
    store
        .transact(
            TxnMeta::by(Author::Human),
            vec![Edit::Set {
                doc: id.clone(),
                id: RecordId::parse("c_b").unwrap(),
                path: FieldPath::field("actors"),
                value: Some(json!(["@actors/e_radio.scene#sc_radio"])),
            }],
        )
        .unwrap();
    let command = WorldCommand::MoveToCell(MoveToCell {
        entity: "e_radio".into(),
        cell: "c_a".into(),
    });
    let txn = command
        .apply(&mut store, &id, TxnMeta::by(Author::Human))
        .unwrap()
        .txn()
        .unwrap();
    assert!(txn.ops.iter().all(|op| op.doc() == &id));
    let manifest = Manifest::read(store.workspace().doc(id.as_str()).unwrap()).unwrap();
    let index = TextIndex::build(&manifest, store.workspace(), store.schema()).unwrap();
    assert_eq!(index.entity("e_radio").unwrap().cell, "c_a");
    assert_eq!(
        index.entity("e_radio").unwrap().reference,
        "@actors/e_radio.scene#e_radio"
    );
}

#[test]
fn disjoint_cell_edits_and_different_entities_in_one_cell_merge_cleanly() {
    let (store, _) = fixture();
    let db = store.schema();
    for second_doc in ["sc_a", "sc_b"] {
        let base = store.workspace().doc("sc_a").unwrap();
        let mut ours = base.clone();
        let mut theirs = base.clone();
        ours.apply(
            Edit::Set {
                doc: base.id.clone(),
                id: RecordId::parse("e_tree").unwrap(),
                path: FieldPath::field("name"),
                value: Some("Oak".into()),
            },
            db,
        )
        .unwrap();
        if second_doc == "sc_a" {
            theirs
                .apply(
                    Edit::Set {
                        doc: base.id.clone(),
                        id: RecordId::parse("e_rock").unwrap(),
                        path: FieldPath::field("name"),
                        value: Some("Granite".into()),
                    },
                    db,
                )
                .unwrap();
        } else {
            let b = store.workspace().doc(second_doc).unwrap();
            let mut edited = b.clone();
            edited
                .apply(
                    Edit::Set {
                        doc: b.id.clone(),
                        id: RecordId::parse("e_door").unwrap(),
                        path: FieldPath::field("name"),
                        value: Some("Gate".into()),
                    },
                    db,
                )
                .unwrap();
            assert!(
                ara_merge::merge(b, b, &edited, db)
                    .unwrap()
                    .conflicts
                    .is_empty()
            );
        }
        let merged = ara_merge::merge(base, &ours, &theirs, db).unwrap();
        assert!(merged.conflicts.is_empty());
        assert_eq!(merged.doc.record("e_tree").unwrap()["name"], "Oak");
        if second_doc == "sc_a" {
            assert_eq!(merged.doc.record("e_rock").unwrap()["name"], "Granite");
        }
    }
}

#[test]
fn cell_cook_checksum_hierarchy_and_pack_boundaries() {
    let (store, id) = fixture();
    let manifest = Manifest::read(store.workspace().doc(id.as_str()).unwrap()).unwrap();
    let a = cook_cell(&manifest, "c_a", store.workspace(), store.schema(), None).unwrap();
    let b = cook_cell(&manifest, "c_b", store.workspace(), store.schema(), None).unwrap();
    let bytes = a.encode().unwrap();
    assert_eq!(CellBlob::decode(&bytes).unwrap().entities.len(), 2);
    let mut corrupt = bytes;
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    assert!(CellBlob::decode(&corrupt).is_err());
    let input = a.pack_input().unwrap();
    assert_eq!(input.path, "world/cells/c_a.aracell");
    let cook = ara_pack::CookKey {
        platform: "windows".into(),
        arch: "x86_64".into(),
        config: "test".into(),
        cooker: "world/1".into(),
        settings: Default::default(),
    };
    let pack = ara_pack::build([input, b.pack_input().unwrap()], cook.clone(), 4096, None).unwrap();
    assert_eq!(pack.manifest.entries.len(), 2);
    assert_ne!(
        pack.manifest.entries["world/cells/c_a.aracell"].chunks,
        pack.manifest.entries["world/cells/c_b.aracell"].chunks
    );
    let source = PackSource::new(std::sync::Arc::from(pack.bytes.as_slice()), None).unwrap();
    assert_eq!(source.load("c_a", 1024 * 1024).unwrap().entities.len(), 2);
    let mut edited = a.clone();
    edited.entities[0]
        .record
        .insert("name".into(), "Changed".into());
    let changed = ara_pack::build(
        [edited.pack_input().unwrap(), b.pack_input().unwrap()],
        cook,
        4096,
        None,
    )
    .unwrap();
    let delta = ara_pack::diff(&pack.manifest, &changed.manifest).unwrap();
    assert!(
        pack.manifest.entries["world/cells/c_b.aracell"]
            .chunks
            .iter()
            .all(|id| !delta.added.contains(id) && !delta.removed.contains(id))
    );
    let mut self_parent = a;
    let identity = self_parent.entities[0].identity.to_owned();
    self_parent.entities[0]
        .record
        .insert("parent".into(), identity.into());
    assert!(self_parent.encode().is_err());
}

#[test]
fn prefab_actor_index_and_cooking_preserve_world_positions_and_hlod_descriptors() {
    let (mut store, world) = fixture();
    let prefab = document(
        "prefab",
        "pf_tree",
        &[
            (
                "e_root",
                json!({"Transform":{"position":[10,0,20],"scale":[2,2,2]}}),
            ),
            (
                "e_leaf",
                json!({"parent":"e_root","order":"a0","Transform":{"position":[1,0,0]}}),
            ),
        ],
    );
    store.load_doc("prefabs/tree.prefab", prefab);
    store
        .transact(
            TxnMeta::by(Author::Human),
            vec![Edit::Create {
                doc: AssetId::parse("sc_a").unwrap(),
                id: RecordId::parse("e_prefab").unwrap(),
                record: serde_json::from_value(json!({"prefab":"@prefabs/tree.prefab#pf_tree"}))
                    .unwrap(),
            }],
        )
        .unwrap();
    let manifest = Manifest::read(store.workspace().doc(world.as_str()).unwrap()).unwrap();
    let index = TextIndex::build(&manifest, store.workspace(), store.schema()).unwrap();
    assert_eq!(
        index.entity("e_prefab").unwrap().position,
        [10.0, 0.0, 20.0]
    );
    struct Proxies;
    impl HlodCooker for Proxies {
        fn cook(&self, _: &CellBlob) -> ara_world::Result<Vec<HlodProxy>> {
            Ok(vec![HlodProxy {
                asset: "@hlod/tree.mesh#ms_proxy".into(),
                min: [0.0; 3],
                max: [250.0; 3],
                error: 0.5,
            }])
        }
    }
    let blob = cook_cell(
        &manifest,
        "c_a",
        store.workspace(),
        store.schema(),
        Some(&Proxies),
    )
    .unwrap();
    assert_eq!(blob.entities.len(), 4);
    assert_eq!(blob.proxies.len(), 1);
    let child = blob
        .entities
        .iter()
        .find(|e| e.record.get("parent") == Some(&json!("e_prefab")))
        .unwrap();
    assert_eq!(child.record["order"], "a0");
    assert!(child.identity.starts_with("e_"));
}

#[test]
fn runtime_layer_set_is_an_undoable_attributed_transaction() {
    let (mut store, id) = fixture();
    let txn = WorldCommand::SetLayer(SetLayer {
        layer: "l_gameplay".into(),
        loaded: false,
        visible: false,
    })
    .apply(&mut store, &id, TxnMeta::by(Author::Human))
    .unwrap()
    .txn()
    .unwrap()
    .id
    .clone();
    let manifest = Manifest::read(store.workspace().doc(id.as_str()).unwrap()).unwrap();
    assert!(!manifest.layers["l_gameplay"].loaded);
    store.undo(&txn, Author::Human).unwrap();
    assert!(
        Manifest::read(store.workspace().doc(id.as_str()).unwrap())
            .unwrap()
            .layers["l_gameplay"]
            .loaded
    );
}
