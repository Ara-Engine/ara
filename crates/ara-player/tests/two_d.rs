#![allow(clippy::unwrap_used)]
//! A 2D scene in Play, driven over MCP: the ara-render2d platformer sample (sprites,
//! flipbooks, a tilemap, 2D lights, Rapier2D bodies) imported through the asset
//! pipeline (atlas importer in the worker), played with the scout as a 2D player
//! character and captured through ara-render2d.
mod support;

use ara_asset::{ImportSettings, Importer, Sidecar, SidecarCommand};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};
use support::Agent;

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ara-render2d/sample")
}

/// A project around the sample: its scene, the two atlas PNGs and their sidecars
/// (authored with typed `SidecarCommand`s and written canonically).
fn project() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-2d-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("levels")).unwrap();
    std::fs::create_dir_all(dir.join("art")).unwrap();
    std::fs::write(
        dir.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"name": "Platformer", "main": "levels/platformer.scene"}),
        ),
    )
    .unwrap();
    std::fs::copy(
        sample().join("platformer.scene"),
        dir.join("levels/platformer.scene"),
    )
    .unwrap();
    let settings: Value = serde_json::from_str(
        &std::fs::read_to_string(sample().join("atlas.settings.json")).unwrap(),
    )
    .unwrap();
    let mut schema = ara_schema::SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_asset::register_schema(&mut schema);
    for (png, id) in [
        ("platformer.png", "as_platform1"),
        ("platformer-normal.png", "as_normal01"),
    ] {
        std::fs::copy(sample().join(png), dir.join("art").join(png)).unwrap();
        let id = ara_doc::AssetId::parse(id).unwrap();
        let mut workspace = ara_doc::Workspace::default();
        workspace.insert("sidecar", Sidecar::empty(id.clone()).into_document());
        let mut store = ara_ops::Store::new(schema.clone(), workspace);
        let edit = SidecarCommand::Initialize(ImportSettings {
            importer: "ara.atlas2d".into(),
            version: ara_import2d::AtlasImporter.version().into(),
            settings: settings.clone(),
            ..Default::default()
        })
        .edit(id.clone())
        .unwrap();
        store
            .transact(ara_ops::TxnMeta::by(ara_ops::Author::Human), [edit])
            .unwrap();
        let text = ara_doc::write(store.workspace().doc(id.as_str()).unwrap(), store.schema());
        std::fs::write(dir.join("art").join(format!("{png}.import.json")), text).unwrap();
    }
    dir
}

fn read(agent: &mut Agent, world: &Value, path: &str) -> Value {
    agent.play("read", json!({"world": world, "paths": [path]}))["values"][path].clone()
}

#[test]
fn agent_plays_and_captures_a_2d_scene() {
    let dir = project();
    let captures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-polish/captures");
    let mut agent = Agent::open(&dir, 11, |_| {});
    // The scout becomes the player's 2D character.
    agent.tool(
        "ara_apply",
        json!({"doc": "levels/platformer.scene", "why": "Play as the scout", "commands": [
            {"cmd": "component.add", "args": {"entity": "e_hero1", "component": "PlayerControl",
                "value": {"walkSpeed": 3}}}
        ]}),
    );
    agent.pool_ready();
    let t = Instant::now();
    let world = agent.play("start", json!({"startPaused": true, "seed": 11}));
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;
    let id = world["world"].clone();
    // Let the scout land, then walk right for 1.5 s.
    agent.play("step", json!({"world": id, "ticks": 30}));
    let x0 = read(&mut agent, &id, "e_hero1.PhysicsPose2d.position.x");
    agent.play(
        "step",
        json!({"world": id, "ticks": 90, "inputs": [{"action": "Move", "value": [1, 0], "holdTicks": 90}]}),
    );
    let x1 = read(&mut agent, &id, "e_hero1.PhysicsPose2d.position.x");
    let walked = x1.as_f64().unwrap() - x0.as_f64().unwrap();
    assert!(walked > 2.0, "the scout walked right: {x0} -> {x1}");
    let path = captures.join("platformer.png");
    let t = Instant::now();
    let captured = agent.play(
        "capture",
        json!({"world": id, "path": path.to_string_lossy(), "marks": 16}),
    );
    let capture_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(path.exists());
    let marks = captured["marks"].as_array().unwrap();
    assert!(
        marks.iter().any(|m| m["label"] == "Scout"),
        "labelled 2D capture: {captured}"
    );
    agent.play("stop", json!({"world": id}));
    println!(
        "TWO_D {}",
        serde_json::to_string_pretty(&json!({
            "firstPlayIncludingAtlasImportMs": first_ms,
            "scoutWalkedM": walked,
            "captureMs": capture_ms,
            "marks": marks.len(),
        }))
        .unwrap()
    );
}

#[test]
#[ignore = "opens a native 2D player window"]
fn two_d_scene_presents_in_a_window() {
    let dir = project();
    let mut agent = Agent::open(&dir, 11, |config| {
        config.pool.windowed = 1;
        config.pool.headless = 0;
    });
    agent.until("a warm window", |a| {
        a.play("status", json!({}))["pool"]["windowed"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    });
    let started = agent.play("start", json!({"window":true}));
    let id = started["world"].clone();
    agent.until("native 2D frames", |a| {
        let status = a.play("status", json!({}));
        let player = &status["worlds"][0]["players"][0]["status"];
        assert!(player["fault"].is_null(), "{status}");
        player["fps"].as_f64().unwrap_or(0.0) > 0.0 && player["tick"].as_u64().unwrap_or(0) > 30
    });
    let path = support::repo().join("target/runtime-polish/2d-window.png");
    agent.play(
        "capture",
        json!({"world":id,"path":path,"marks":0,"presented":true}),
    );
    agent.play("stop", json!({"world":id}));
}
