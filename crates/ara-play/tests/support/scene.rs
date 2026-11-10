//! Shared deterministic CPU-only transfer/projection fixture, including prefab dependencies.
#![allow(dead_code)]
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_play::PlayScene;
use ara_schema::SchemaDb;
use ara_sim::{EditWorld, SimRegistry};
use serde_json::json;

pub fn schema() -> SchemaDb {
    let mut schema = SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_components::register(&mut schema);
    assert!(
        schema
            .add_json_types(&json!({"$schema":"ara://schema/types/1","types":{
                "Stats":{"kind":"component","doc":"Variable scene data.","fields":{
                    "hp":{"type":"i32","doc":"Health.","default":100},
                    "title":{"type":"string","doc":"Display title."},
                    "items":{"type":"list<u64>","doc":"Inventory IDs."},
                    "direction":{"type":"vec3","doc":"Direction."},
                    "enabled":{"type":"bool","doc":"Active."}
                }}
            }}))
            .is_empty()
    );
    schema
}

pub fn source(rows: usize) -> Workspace {
    let mut workspace = Workspace::default();
    let mut scene = Document::new("scene", AssetId::parse("sc_bench").expect("scene ID"));
    for i in 0..rows {
        let record = if i > 0 && i.is_multiple_of(16) {
            json!({"name":format!("Prefab {i}"),"prefab":"@templates/actor.prefab#pf_bench","order":"a0"})
        } else {
            let mut record = json!({"name":format!("Actor {i}: café \\\""),"order":"a0",
                "Transform":{"position":[i as f32 * 0.125,-0.0,0.1]},
                "Stats":{"hp":100,"title":format!("Inventory {i}"),"items":[1,5,8,13],
                    "direction":[0.1,0.2,0.3],"enabled":true}});
            if i > 0 && i.is_multiple_of(8) {
                record["parent"] = json!(format!("e_{:08x}", i - 1));
            }
            record
        };
        scene.insert_unchecked(
            RecordId::parse(&format!("e_{i:08x}")).expect("record ID"),
            record.as_object().expect("record object").clone(),
        );
    }
    let mut prefab = Document::new("prefab", AssetId::parse("pf_bench").expect("prefab ID"));
    for (id, record) in [
        (
            "e_0001",
            json!({"name":"Template root","Transform":{},"Stats":{"title":"Prefab","items":[2,3]}}),
        ),
        (
            "e_0002",
            json!({"parent":"e_0001","order":"a1","name":"Template child","Transform":{}}),
        ),
    ] {
        prefab.insert_unchecked(
            RecordId::parse(id).expect("prefab record ID"),
            record.as_object().expect("record").clone(),
        );
    }
    workspace.insert("levels/bench.scene", scene);
    workspace.insert("templates/actor.prefab", prefab);
    workspace
}

pub fn snapshot(workspace: &Workspace) -> PlayScene {
    let mut scene: PlayScene = serde_json::from_value(json!({
        "format":1,"scene":"sc_bench","documents":[],"types":[],"seed":901,"tick_hz":60,
        "assets":{"as_beef":"ab".repeat(32)},"cache":"target/scene-preparation/cache",
        "nav_cache":"target/scene-preparation/nav","imports":{"as_beef":"audio/effect.wav"},
        "scripts":{"hostDir":"host","compileServer":"compiler.dll","modules":["game"]},
        "ui":{"markup":"<div>Loading café</div>","stylesheet":"div { color: white; }","bindings":[],
            "fonts":[[1,2,3]],"images":{"icon":[4,5,6]},"click":[7,8,9],
            "resource_paths":{"image:icon":"ui/icon.png"},"resource_diagnostics":["sample warning"]}
    }))
    .expect("play snapshot");
    scene.documents = PlayScene::documents(workspace);
    scene
}

/// Frozen original borrowed reconstruction, independent of the optimized API.
pub fn borrowed_workspace(scene: &PlayScene) -> Result<(Workspace, AssetId), String> {
    let mut index = ara_doc::refs::AssetIndex::new();
    for (id, path) in &scene.imports {
        index.insert(id.clone(), path.clone(), None);
    }
    let mut workspace = Workspace::new(index);
    for data in &scene.documents {
        let mut doc = Document::new(data.kind.clone(), AssetId::parse(&data.id)?);
        for (id, record) in &data.records {
            doc.insert_unchecked(RecordId::parse(id)?, record.clone());
        }
        workspace.insert(&data.path, doc);
    }
    Ok((workspace, AssetId::parse(&scene.scene)?))
}

pub fn project(schema: &SchemaDb, workspace: Workspace, id: AssetId) -> EditWorld {
    EditWorld::from_workspace(
        SimRegistry::new(schema.clone()).expect("registry"),
        workspace,
        id,
    )
    .expect("scene projection")
}

pub fn change(edit: &mut EditWorld) {
    let op = ara_doc::Op::Set {
        doc: AssetId::parse("sc_bench").expect("scene ID"),
        id: RecordId::parse("e_00000000").expect("record ID"),
        path: "Stats.hp".parse().expect("field path"),
        value: Some(json!(73)),
        prev: Some(json!(100)),
    };
    edit.apply_ops(&[op]).expect("baseline-checked live change");
}
