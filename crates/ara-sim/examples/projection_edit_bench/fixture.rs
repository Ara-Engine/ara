use ara_doc::{AssetId, Document, Edit, Op, RecordId, Workspace};
use ara_schema::SchemaDb;
use serde_json::json;

pub fn fixture(rows: usize, case: &str) -> (SchemaDb, Workspace, Workspace, Vec<Op>) {
    let mut schema = SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_components::register(&mut schema);
    assert!(
        schema
            .add_json_types(&json!({"$schema":"ara://schema/types/1","types":{
                "Stats":{"kind":"component","doc":"Mutable actor data.","fields":{
                    "hp":{"type":"i32","doc":"Health.","default":100},
                    "title":{"type":"string","doc":"Title."},
                    "items":{"type":"list<u64>","doc":"Inventory."}
                }}
            }}))
            .is_empty()
    );
    let prefab = case.starts_with("prefab-");
    let mut workspace = Workspace::default();
    let mut doc = Document::new(
        if prefab { "prefab" } else { "scene" },
        AssetId::parse(if prefab { "pf_bench" } else { "sc_bench" }).expect("asset ID"),
    );
    for i in 0..rows {
        let mut record = json!({"name":format!("Actor {i}"),"order":"a0",
            "Transform":{"position":[i as f32 * 0.125,-0.0,0.1]},
            "Stats":{"hp":100,"title":format!("Inventory {i}"),"items":[1,5,8,13]}});
        if prefab && i != 0 {
            record["parent"] = json!("e_00000000");
        }
        doc.insert_unchecked(
            RecordId::parse(&format!("e_{i:08x}")).expect("record ID"),
            record.as_object().expect("record").clone(),
        );
    }
    workspace.insert(
        if prefab {
            "actor.prefab"
        } else {
            "bench.scene"
        },
        doc,
    );
    if prefab {
        let mut scene = Document::new("scene", AssetId::parse("sc_bench").expect("scene ID"));
        for (id, record) in [
            ("e_anchor", json!({"order":"a0","Transform":{},"Stats":{}})),
            (
                "e_instance",
                json!({"order":"a1","name":"Instance","prefab":"@actor.prefab#pf_bench"}),
            ),
        ] {
            scene.insert_unchecked(
                RecordId::parse(id).expect("record ID"),
                record.as_object().expect("record").clone(),
            );
        }
        workspace.insert("bench.scene", scene);
    }
    let mut expected = workspace.clone();
    let mut ops = Vec::new();
    let count = match case {
        "one" | "prefab-name" | "prefab-override" => 1,
        "pct1" => (rows / 100).max(1),
        "pct10" => (rows / 10).max(1),
        "all" => rows,
        _ => panic!("--case one|pct1|pct10|all|prefab-name|prefab-override"),
    };
    for i in 0..count {
        let (id, path, value) = match case {
            "prefab-name" => ("e_instance".to_owned(), "name", json!("Edited instance")),
            "prefab-override" => (
                "e_instance".to_owned(),
                "overrides[e_00000001/Stats.hp]",
                json!(73),
            ),
            _ => (format!("e_{i:08x}"), "Stats.hp", json!(73)),
        };
        let op = expected
            .doc_mut("sc_bench")
            .expect("scene")
            .apply(
                Edit::Set {
                    doc: AssetId::parse("sc_bench").expect("scene ID"),
                    id: RecordId::parse(&id).expect("record ID"),
                    path: path.parse().expect("path"),
                    value: Some(value),
                },
                &schema,
            )
            .expect("real edit with recorded preimage");
        ops.push(op);
    }
    (schema, workspace, expected, ops)
}
