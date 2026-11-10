use super::*;
use ara_schema::Schema;
use serde::{Deserialize, Deserializer, Serialize};

/// A typed component whose storage codec rejects a schema-valid sentinel.
#[derive(Component, Schema, Serialize, Deserialize)]
#[ara(component)]
pub(super) struct Guarded {
    /// A value whose sentinel tests deserialization preflight.
    #[serde(deserialize_with = "guard")]
    pub value: i32,
}
fn guard<'de, D: Deserializer<'de>>(decoder: D) -> std::result::Result<i32, D::Error> {
    let value = i32::deserialize(decoder)?;
    if value == 13 {
        return Err(serde::de::Error::custom("thirteen is rejected by storage"));
    }
    Ok(value)
}

pub(super) fn fixture(external: bool) -> EditWorld {
    let mut db = ara_schema::SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    assert!(
        db.add_json_types(&json!({"$schema":"ara://schema/types/1","types":{
            "Stats":{"kind":"component","doc":"Runtime stats.","fields":{
                "hp":{"type":"i32","doc":"Health.","default":100},
                "title":{"type":"string","doc":"Title."},
                "target":{"type":"entity","doc":"Reference."}
            }}
        }}))
        .is_empty()
    );
    let mut registry = SimRegistry::new(db).expect("registry");
    registry
        .register_component::<Guarded>()
        .expect("typed storage");
    if external {
        registry
            .register_external("Stats")
            .expect("external storage");
    }
    let mut workspace = Workspace::default();
    for (path, kind, id, records) in [
        (
            "inner.prefab",
            "prefab",
            "pf_inner",
            json!({
                "e_a":{"name":"Inner","Transform":{},"Stats":{"hp":5}},
                "e_c":{"parent":"e_a","name":"Inner child","Transform":{},"Stats":{"target":"e_a"}}
            }),
        ),
        (
            "outer.prefab",
            "prefab",
            "pf_outer",
            json!({
                "e_a":{"name":"Outer","Transform":{},"Stats":{"hp":10}},
                "e_b":{"parent":"e_a","prefab":"@inner.prefab#pf_inner","order":"a1"}
            }),
        ),
        (
            "test.scene",
            "scene",
            "sc_test",
            json!({
                "e_1":{"order":"a0","name":"Root","Transform":{},"Stats":{"hp":100,"title":"root"}},
                "e_2":{"parent":"e_1","order":"a1","Transform":{},"Stats":{"hp":50},"Guarded":{"value":0}},
                "e_3":{"order":"a2","name":"Instance","prefab":"@outer.prefab#pf_outer"},
                "e_4":{"parent":"e_3/e_b","Transform":{},"Stats":{}},
                "e_10":{"order":"a3","Transform":{},"Stats":{"hp":90}},
                "e_30":{"order":"a4","prefab":"@outer.prefab#pf_outer"}
            }),
        ),
    ] {
        let mut doc = Document::new(kind, AssetId::parse(id).expect("asset ID"));
        for (id, record) in records.as_object().expect("records") {
            doc.insert_unchecked(
                RecordId::parse(id).expect("record ID"),
                record.as_object().expect("record").clone(),
            );
        }
        workspace.insert(path, doc);
    }
    EditWorld::from_workspace(
        registry,
        workspace,
        AssetId::parse("sc_test").expect("scene ID"),
    )
    .expect("valid projection")
}

pub(super) fn set(edit: &EditWorld, doc: &str, id: &str, path: &str, value: Option<Value>) -> Op {
    edit.workspace
        .doc(doc)
        .expect("document")
        .clone()
        .apply(
            Edit::Set {
                doc: AssetId::parse(doc).expect("asset ID"),
                id: RecordId::parse(id).expect("record ID"),
                path: path.parse().expect("field path"),
                value,
            },
            &edit.registry.schema,
        )
        .expect("schema-valid edit")
}

/// A no-op structural edit selects the existing full path without changing the
/// source or requiring a production switch for tests.
pub(super) fn full_reference(edit: &mut EditWorld, ops: &[Op]) -> Result<()> {
    let mut ops = ops.to_vec();
    let order = ara_doc::OrderKey::parse("a0").expect("order");
    ops.push(Op::Move {
        doc: AssetId::parse("sc_test").expect("scene ID"),
        id: RecordId::parse("e_1").expect("record ID"),
        parent: None,
        order: Some(order.clone()),
        prev_parent: None,
        prev_order: Some(order),
    });
    edit.apply_ops(&ops)
}

pub(super) fn comparable(edit: &EditWorld) -> Value {
    let mut rows = BTreeMap::new();
    for entity in edit.world.iter_entities() {
        let Some(id) = entity.get::<Identity>() else {
            continue;
        };
        let mut components = BTreeMap::new();
        for name in edit.ids.keys() {
            if let Some(value) = edit
                .registry
                .read(&edit.world, &edit.ids, entity.id(), name)
                .expect("component read")
            {
                components.insert(name.clone(), value);
            }
        }
        let parent = entity
            .get::<ChildOf>()
            .and_then(|p| edit.world.get::<Identity>(p.parent()))
            .map(|id| &id.0);
        rows.insert(id.0.clone(),json!({"components":components,"metadata":entity.get::<SceneMetadata>(),"parent":parent}));
    }
    json!(rows)
}

pub(super) fn same(a: &EditWorld, b: &EditWorld) {
    assert_eq!(a.resolved, b.resolved, "complete authored baseline");
    assert_eq!(a.mapping, b.mapping, "mapping cleanup and handles");
    for (path, doc) in a.workspace.docs() {
        let other = b.workspace.doc(doc.id.as_str()).expect("same document");
        assert_eq!(b.workspace.path_of(doc.id.as_str()), Some(path));
        assert_eq!(doc, other, "mirrored source");
    }
    assert_eq!(
        comparable(a),
        comparable(b),
        "all runtime component and hierarchy values"
    );
    assert_eq!(
        a.world.resource::<ExternalWrites>().0,
        b.world.resource::<ExternalWrites>().0,
        "ordered external writes"
    );
}

/// One large instance plus the fixture's independent plain rows.
pub(super) fn large_instance(rows: usize) -> EditWorld {
    let edit = fixture(false);
    let mut workspace = edit.workspace.clone();
    let schema = &edit.registry.schema;
    workspace
        .doc_mut("sc_test")
        .expect("scene")
        .apply(
            Edit::Delete {
                doc: AssetId::parse("sc_test").expect("doc"),
                id: RecordId::parse("e_30").expect("id"),
            },
            schema,
        )
        .expect("remove second instance");
    let prefab = workspace.doc_mut("pf_inner").expect("prefab");
    for i in 0..rows {
        prefab.insert_unchecked(
            RecordId::parse(&format!("e_{i:08x}")).expect("id"),
            json!({"parent":"e_a","order":"a0","Transform":{},"Stats":{"hp":100}})
                .as_object()
                .expect("record")
                .clone(),
        );
    }
    EditWorld::from_workspace(edit.registry, workspace, edit.scene).expect("large instance")
}
