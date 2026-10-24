//! Select source kit nodes and switch/recolor a lamp through MCP in real Play.
#![allow(clippy::unwrap_used)]
mod support;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use support::{Agent, demo_copy};

fn capture(agent: &mut Agent, world: &Value, root: &Path, name: &str) -> [f64; 3] {
    let path = root.join(format!("{name}.png"));
    let result = agent.play(
        "capture",
        json!({"world":world,"path":path.to_string_lossy(),
        "width":320,"height":240,"marks":1}),
    );
    assert!(
        result["marks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["label"]
                .as_str()
                .unwrap_or_default()
                .contains("Selected lamp")),
        "the selected source node must be visible at its restored pivot: {result}"
    );
    let image = image::open(path).unwrap().into_rgb8();
    let mut mean = [0.; 3];
    for pixel in image.pixels() {
        for channel in 0..3 {
            mean[channel] += f64::from(pixel[channel]);
        }
    }
    mean.map(|sum| sum / f64::from(image.width() * image.height()))
}

fn patch(agent: &mut Agent, world: &Value, path: &str, value: Value) {
    agent.play(
        "patch",
        json!({"world":world,"sets":[{"entity":"e_lamp","path":path,"value":value}]}),
    );
}

#[test]
fn agent_selects_a_kit_piece_and_toggles_its_source_emission_without_derived_gltfs() {
    let dir = demo_copy("model-authoring");
    std::fs::create_dir_all(dir.join("models")).unwrap();
    let mut source: Value = serde_json::from_slice(include_bytes!(
        "../../ara-importers/tests/fixtures/material-extensions.gltf"
    ))
    .unwrap();
    source["nodes"] = json!([
        {"name":"LampA","mesh":0,"translation":[10,0,0]},
        {"name":"LampB","mesh":0,"translation":[100,0,0]}
    ]);
    source["scenes"] = json!([{"nodes":[0,1]}]);
    std::fs::write(
        dir.join("models/kit.gltf"),
        ara_json::to_canonical_string(&source),
    )
    .unwrap();
    let mut agent = Agent::open(&dir, 81, |_| {});
    agent.tool("ara_apply", json!({"why":"Create a scene using the original model kit", "commands":[
        {"cmd":"doc.create","args":{"path":"levels/parts.scene","id":"sc_parts"}},
        {"cmd":"import.initialize","args":{"source":"models/kit.gltf","id":"as_kit","settings":{"virtualGeometry":false}}}
    ]}));
    agent.tool("ara_apply", json!({"doc":"levels/parts.scene","why":"Frame and illuminate the selected kit piece", "commands":[
        {"cmd":"entity.create","args":{"id":"e_camera","components":{"Transform":{"position":[0.5,1.7,2.1]},"Camera":{"fov":50}}}},
        {"cmd":"transform.lookAt","args":{"entity":"e_camera","point":[0.5,0,0.5]}},
        {"cmd":"entity.create","args":{"id":"e_lamp","name":"Selected lamp","components":{"Transform":{},
            "MeshRenderer":{"model":"@models/kit.gltf#as_kit","node":"LampA","emissiveFactor":[5000,0,0],
                "emissiveMaterials":["Layered"]}}}},
        {"cmd":"physics.collider.fromModel","args":{"entity":"e_lamp","mode":"mesh"}},
        {"cmd":"entity.create","args":{"id":"e_proxy","components":{"Transform":{"position":[4,0,0]},"RigidBody":{},"Collider":{}}}}
    ]}));
    agent.pool_ready();
    let world = agent.play(
        "start",
        json!({"scene":"levels/parts.scene","startPaused":true,"seed":81}),
    )["world"]
        .clone();
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/authoring-tests/model-authoring");
    let ids = agent.play(
        "read",
        json!({"world":world,"paths":["e_lamp.PhysicsId.value","e_proxy.PhysicsId.value"]}),
    )["values"]
        .clone();
    let lamp = ids["e_lamp.PhysicsId.value"].as_u64().unwrap();
    let proxy = ids["e_proxy.PhysicsId.value"].as_u64().unwrap();
    assert!(lamp > 0 && proxy > 0 && lamp != proxy, "{ids}");
    std::fs::create_dir_all(&output).unwrap();
    let red = capture(&mut agent, &world, &output, "red");
    patch(
        &mut agent,
        &world,
        "MeshRenderer.emissiveFactor",
        json!([0, 5000, 0]),
    );
    let green = capture(&mut agent, &world, &output, "green");
    assert!(
        red[0] > green[0] + 2.,
        "factor edits must invalidate cached materials: red={red:?}, green={green:?}"
    );
    assert!(green[1] > red[1] + 2., "red={red:?}, green={green:?}");
    patch(
        &mut agent,
        &world,
        "MeshRenderer.emissiveEnabled",
        json!(false),
    );
    let off = capture(&mut agent, &world, &output, "off");
    assert!(
        green.iter().sum::<f64>() > off.iter().sum::<f64>() + 4.,
        "on={green:?}, off={off:?}"
    );
    patch(
        &mut agent,
        &world,
        "MeshRenderer.emissiveEnabled",
        json!(true),
    );
    patch(&mut agent, &world, "MeshRenderer.node", json!("LampB"));
    let sibling = capture(&mut agent, &world, &output, "sibling");
    for channel in 0..3 {
        assert!(
            (sibling[channel] - green[channel]).abs() < 0.1,
            "each selected source pivot restores the same local geometry: {sibling:?}, {green:?}"
        );
    }
    agent.play("stop", json!({"world":world}));
    println!(
        "MODEL_AUTHORING {:?}: red={red:?}, green={green:?}, off={off:?}",
        output
    );
}

#[test]
fn agent_swaps_native_material_assets_on_a_procedural_primitive_in_play() {
    let dir = demo_copy("native-material-authoring");
    let mut agent = Agent::open(&dir, 82, |_| {});
    agent.tool(
        "ara_apply",
        json!({"why":"Author native material assets", "commands":[
            {"cmd":"doc.create","args":{"path":"levels/material.scene","id":"sc_material"}},
            {"cmd":"doc.create","args":{"kind":"material","path":"red.graph","id":"gr_red"}},
            {"cmd":"doc.create","args":{"kind":"material","path":"green.graph","id":"gr_green"}}
        ]}),
    );
    for (path, rgb) in [("red.graph", "5000, 0, 0"), ("green.graph", "0, 5000, 0")] {
        agent.tool("ara_apply",json!({"why":"Author the lamp emission", "doc":path,"commands":[{"cmd":"material.text.apply","args":{"text":format!("material Lamp : surface budget 1 {{ let light : ColorLinear = linear({rgb}); output emissive = light; }}")}}]}));
    }
    agent.tool("ara_apply",json!({"why":"Place a lamp using its native material", "doc":"levels/material.scene","commands":[
        {"cmd":"entity.create","args":{"id":"e_camera","components":{"Transform":{"position":[0,2,2]},"Camera":{"fov":50}}}},
        {"cmd":"transform.lookAt","args":{"entity":"e_camera","point":[0,0,0]}},
        {"cmd":"entity.create","args":{"id":"e_lamp","name":"Selected lamp","components":{"Transform":{},"MeshRenderer":{"shape":"plane","size":[2,1,2],"materialOverrides":[{"primitive":0,"material":"@red.graph#gr_red"}]}}}}
    ]}));
    agent.pool_ready();
    let world = agent.play(
        "start",
        json!({"scene":"levels/material.scene","startPaused":true,"seed":82}),
    )["world"]
        .clone();
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/authoring-tests/native-material");
    std::fs::create_dir_all(&output).unwrap();
    let red = capture(&mut agent, &world, &output, "red");
    patch(
        &mut agent,
        &world,
        "MeshRenderer.materialOverrides",
        json!([{"primitive":0,"material":"@green.graph#gr_green"}]),
    );
    let green = capture(&mut agent, &world, &output, "green");
    assert!(
        red[0] > green[0] + 2. && green[1] > red[1] + 2.,
        "red={red:?}, green={green:?}"
    );
    patch(
        &mut agent,
        &world,
        "MeshRenderer.emissiveEnabled",
        json!(false),
    );
    let off = capture(&mut agent, &world, &output, "off");
    assert!(
        green.iter().sum::<f64>() > off.iter().sum::<f64>() + 4.,
        "green={green:?}, off={off:?}"
    );
    agent.play("stop", json!({"world":world}));
}
