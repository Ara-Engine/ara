#![allow(clippy::unwrap_used)]
//! C# gameplay in Play, driven by an agent over MCP. The Outpost module (Door
//! behaviour, UseMedkit command, Gravity and Movement systems) loads in the player;
//! the owner publishes its types so the host validates C# components; the agent adds
//! them to the scene, plays, runs a command, edits the module's source while playing
//! (hot reload at a tick boundary) and restarts keeping C# state. Prints numbers.
mod support;

use ara_play_host::ScriptPaths;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use support::{Agent, copy, demo_copy};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Builds `dotnet/Ara.slnx` (Release): the JIT host, compile server and analyzers.
fn build_dotnet() {
    let out = Command::new("dotnet")
        .args(["build", "Ara.slnx", "-c", "Release", "--nologo", "-v", "q"])
        .current_dir(repo().join("dotnet"))
        .output()
        .expect("run `dotnet build` (is the .NET 10 SDK installed?)");
    assert!(
        out.status.success(),
        "dotnet build failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn read(agent: &mut Agent, world: &Value, path: &str) -> Value {
    agent.play("read", json!({"world": world, "paths": [path]}))["values"][path].clone()
}

#[test]
fn agent_hot_reloads_csharp_gameplay_while_playing() {
    build_dotnet();
    let dir = demo_copy("csharp");
    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("demo-scripts/Outpost");
    copy(&module, &dir.join("scripts/Outpost"));
    let scripts = ScriptPaths::locate(&repo()).expect("built .NET host");
    let configure = |config: &mut ara_play_host::PlayConfig| config.scripts = Some(scripts.clone());

    // 1. The first Play loads the module in a player; the owner publishes its types.
    let mut agent = Agent::open(&dir, 5, configure);
    agent.pool_ready();
    let t = Instant::now();
    let warm = agent.play("start", json!({"startPaused": true}));
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;
    let types = dir.join("scripts/Outpost/Outpost.types.json");
    agent.until("published C# types", |_| types.exists());
    let checked = module.join("Outpost.types.json");
    if std::env::var_os("ARA_UPDATE_DEMO").is_some() {
        std::fs::copy(&types, &checked).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&checked).unwrap_or_default(),
        std::fs::read_to_string(&types).unwrap(),
        "Outpost.types.json is stale; regenerate with ARA_UPDATE_DEMO=1"
    );
    agent.play("stop", json!({"world": warm["world"]}));
    drop(agent);

    // 2. Reopened, the host validates C# components; the agent adds them.
    let mut agent = Agent::open(&dir, 5, configure);
    let applied = agent.tool(
        "ara_apply",
        json!({"doc": "levels/demo.scene", "why": "C# gameplay", "commands": [
            {"cmd": "component.add", "args": {"entity": "e_fox", "component": "Vitality",
                "value": {"hp": 20, "medkits": 2}}},
            {"cmd": "entity.create", "args": {"id": "$door", "name": "Door", "components": {
                "Transform": {"position": [2, 1, 3]},
                "MeshRenderer": {"size": [1.2, 2, 0.1], "color": [0.5, 0.35, 0.2, 1]},
                "Door": {"openAngle": 100, "isOpen": true}}}},
            {"cmd": "entity.create", "args": {"id": "$drop", "name": "Supply drop", "components": {
                "Transform": {"position": [-2, 6, 3]},
                "MeshRenderer": {"color": [0.9, 0.6, 0.1, 1]},
                "Mass": {"value": 1}, "Motion": {}}}}
        ]}),
    );
    let symbol = |name: &str| {
        applied["ids"][name]
            .as_str()
            .unwrap_or_else(|| panic!("no {name} in {applied}"))
            .to_owned()
    };
    let (door, drop) = (symbol("$door"), symbol("$drop"));

    agent.pool_ready();
    let t = Instant::now();
    let world = agent.play("start", json!({"startPaused": true, "seed": 5}));
    let boot_ms = t.elapsed().as_secs_f64() * 1000.0;
    let id = world["world"].clone();
    let catalog = agent.play("commands", json!({"world": id}));
    assert!(catalog.get("game.UseMedkit").is_some(), "{catalog}");

    // Gravity and Movement drop the crate onto the floor; the Door behaviour swings.
    agent.play("step", json!({"world": id, "ticks": 120}));
    let y = read(&mut agent, &id, &format!("{drop}.Transform.position.y"));
    assert_eq!(y.as_f64(), Some(0.5), "the drop rests on the floor");
    let angle = read(&mut agent, &id, &format!("{door}.Transform.rotation.y"));
    assert_eq!(angle.as_f64(), Some(100.0), "the door opened");

    // UseMedkit heals 25.
    agent.play(
        "command",
        json!({"world": id, "name": "game.UseMedkit", "params": {"actor": "e_fox"}}),
    );
    assert_eq!(read(&mut agent, &id, "e_fox.Vitality.hp"), json!(45));

    // The agent edits the command body while playing: an EnC delta at a tick boundary.
    let source = std::fs::read_to_string(dir.join("scripts/Outpost/Vitals.cs")).unwrap();
    let edited = source.replace("v.Hp + 25", "v.Hp + 40");
    assert_ne!(source, edited);
    let t = Instant::now();
    let written = agent.play(
        "write",
        json!({"path": "scripts/Outpost/Vitals.cs", "text": edited}),
    );
    assert_eq!(written["claimedBy"], json!(1), "{written}");
    let mut reload = Value::Null;
    agent.until("the hot reload", |a| {
        a.play("step", json!({"world": id, "ticks": 1}));
        let events = a.play("events", json!({"world": id}));
        if let Some(r) = events["events"].as_array().and_then(|e| {
            // Level 0 is the initial load; 1–3 are hot reloads.
            e.iter().find(|e| {
                e["kind"] == "reload" && (1..=3).contains(&e["level"].as_u64().unwrap_or(0))
            })
        }) {
            reload = r.clone();
        }
        !reload.is_null()
    });
    let reload_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(reload["errors"], json!(0), "{reload}");
    assert_eq!(reload["origin"], json!(1), "an agent edit: {reload}");
    agent.play(
        "command",
        json!({"world": id, "name": "game.UseMedkit", "params": {"actor": "e_fox"}}),
    );
    assert_eq!(
        read(&mut agent, &id, "e_fox.Vitality.hp"),
        json!(85),
        "new code heals 40"
    );

    // Restart keeping state (what an L4 reload does): C# and engine state survive.
    let t = Instant::now();
    agent.play("restart", json!({"world": id, "keepState": true}));
    let restart_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(read(&mut agent, &id, "e_fox.Vitality.hp"), json!(85));
    assert_eq!(
        read(&mut agent, &id, &format!("{drop}.Transform.position.y")).as_f64(),
        Some(0.5)
    );
    agent.play("stop", json!({"world": id}));
    println!(
        "CSHARP {}",
        serde_json::to_string_pretty(&json!({
            "firstPlayWithDotnetMs": first_ms,
            "bootFromPoolWithModulesMs": boot_ms,
            "hotReload": {"saveToVisibleMs": reload_ms, "level": reload["level"]},
            "restartKeepingStateMs": restart_ms,
        }))
        .unwrap()
    );
}
