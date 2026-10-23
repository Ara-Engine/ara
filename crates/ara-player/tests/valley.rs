#![allow(clippy::unwrap_used)]
//! The Valley over MCP, networked (D19: host + 1 client, real replication): terrain
//! and foliage, a 4-cell streamed world (chunks, props, lamps, baked acoustics), the
//! motion-matched Fox, the guard NPC (behaviour tree on streamed nav v2), the
//! campfire (VFX, crackle, acoustic source), interactive music, and C# gameplay an
//! agent adds through `ara_apply` (the Outpost module: vitality and medkits, a door,
//! a supply drop). The host's player walks east into the next cell; both processes
//! capture their view; the host renders its sound offline. Prints the timings.
mod support;

use ara_play_host::ScriptPaths;
use serde_json::{Value, json};
use std::{path::Path, time::Instant};
use support::{Agent, build_dotnet, copy, demo_copy, repo};

const SCENE: &str = "levels/valley.scene";

/// The status of player `index` (0 = authority) of the latest world.
fn status(agent: &mut Agent, index: usize) -> Value {
    let status = agent.play("status", json!({}));
    status["worlds"][0]["players"][index]["status"].clone()
}

fn until_tick(agent: &mut Agent, tick: u64) {
    agent.until("the authority's tick", |a| {
        status(a, 0)["tick"].as_u64().unwrap_or(0) >= tick
    });
}

fn read(agent: &mut Agent, world: &Value, player: Option<u32>, paths: &[&str]) -> Value {
    let mut args = json!({"world": world, "paths": paths});
    if let Some(p) = player {
        args["player"] = json!(p);
    }
    agent.play("read", args)["values"].clone()
}

fn position(values: &Value, path: &str) -> [f64; 3] {
    let p = &values[path];
    [0, 1, 2].map(|i| p[i].as_f64().unwrap_or(f64::NAN))
}

#[test]
fn valley_plays_networked_with_terrain_streaming_ai_audio_vfx_and_csharp() {
    build_dotnet();
    let dir = demo_copy("valley");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("demo-scripts/Outpost"),
        &dir.join("scripts/Outpost"),
    );
    let scripts = ScriptPaths::locate(&repo()).expect("built .NET host");
    let captures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-polish/captures");
    std::fs::create_dir_all(&captures).unwrap();
    let mut agent = Agent::open(&dir, 31, |config| config.scripts = Some(scripts.clone()));

    // C# gameplay, authored by the agent into the level (camp ground is ~0 m).
    agent.tool(
        "ara_apply",
        json!({"doc": SCENE, "why": "C# gameplay in the valley camp", "commands": [
            {"cmd": "component.add", "args": {"entity": "e_fox", "component": "Vitality",
                "value": {"hp": 40, "medkits": 2}}},
            {"cmd": "entity.create", "args": {"id": "$door", "name": "Supply shed door", "components": {
                "Transform": {"position": [12.2, 1, 12.5]},
                "MeshRenderer": {"size": [1.2, 2, 0.1], "color": [0.5, 0.35, 0.2, 1]},
                "Door": {"openAngle": 100, "isOpen": true}}}},
            {"cmd": "entity.create", "args": {"id": "$drop", "name": "Supply drop", "components": {
                "Transform": {"position": [17, 7, 11]},
                "MeshRenderer": {"color": [0.9, 0.6, 0.1, 1]},
                "Mass": {"value": 1}, "Motion": {}}}}
        ]}),
    );
    agent.pool_ready();

    // Host + 1 client, both processes with the C# modules.
    let t = Instant::now();
    let started = agent.play(
        "start",
        json!({"scene": SCENE, "mode": "host", "clients": 1, "seed": 31}),
    );
    let start_ms = t.elapsed().as_secs_f64() * 1000.0;
    let w = started["world"].clone();
    let host_started = started["players"][0]["started"].clone();
    agent.until("the client's first authoritative tick", |a| {
        status(a, 1)["net"]["joinMs"].is_number()
    });
    agent.until("the camp cell", |a| {
        status(a, 0)["cells"] == json!(["c_00"])
    });
    let begin = status(&mut agent, 0)["tick"].as_u64().unwrap();
    let before = read(
        &mut agent,
        &w,
        None,
        &["e_fox.Transform.position", "e_guard.Transform.position"],
    );

    // The host's player walks east across the cell border (networked locomotion takes
    // `Move` in world axes at walking speed); cell (1, 0) streams in ahead of it.
    agent.play(
        "input",
        json!({"world": w, "inputs": [
            {"action": "Move", "value": [0, 1], "holdTicks": 600, "player": 1}
        ]}),
    );
    until_tick(&mut agent, begin + 600 + 30);
    let paths = [
        "e_fox.Transform.position",
        "e_fox.CharacterMotion.grounded",
        "e_guard.Transform.position",
        "e_fox.Vitality.hp",
    ];
    let host = read(&mut agent, &w, None, &paths);
    let client = read(&mut agent, &w, Some(1), &paths);
    let fox = position(&host, "e_fox.Transform.position");
    assert!(fox[0] > 33.0, "the Fox crossed into cell (1, 0): {host}");
    assert_eq!(
        host["e_fox.CharacterMotion.grounded"],
        json!(true),
        "{host}"
    );
    let host_status = status(&mut agent, 0);
    let cells = host_status["cells"].as_array().unwrap().clone();
    assert!(cells.contains(&json!("c_10")), "{host_status}");
    // Replicated: the client sees the Fox and the guard where the host has them.
    let seen = position(&client, "e_fox.Transform.position");
    assert!(
        (seen[0] - fox[0]).abs() < 0.5 && (seen[2] - fox[2]).abs() < 0.5,
        "client sees the host's Fox: {client} vs {host}"
    );
    let guard = position(&host, "e_guard.Transform.position");
    let guard_was = position(&before, "e_guard.Transform.position");
    assert!(
        (guard[0] - guard_was[0]).abs() + (guard[2] - guard_was[2]).abs() > 2.0,
        "the guard moved: {before} -> {host}"
    );

    // C#: the supply drop fell, the door swung open, a medkit heals.
    let csharp = read(&mut agent, &w, None, &["e_fox.Vitality.hp"]);
    assert_eq!(csharp["e_fox.Vitality.hp"], json!(40), "{csharp}");
    agent.play(
        "command",
        json!({"world": w, "name": "game.UseMedkit", "params": {"actor": "e_fox"}}),
    );
    until_tick(&mut agent, begin + 600 + 40);
    let healed = read(&mut agent, &w, None, &["e_fox.Vitality.hp"]);
    assert_eq!(healed["e_fox.Vitality.hp"], json!(65), "{healed}");
    agent.play(
        "command",
        json!({"world":w,"name":"game.UseMedkit","params":{"actor":"e_player02"}}),
    );
    until_tick(&mut agent, begin + 600 + 65);
    let fields = [
        "e_fox.Vitality.hp",
        "e_fox.Vitality.medkits",
        "e_player02.Vitality.hp",
        "e_player02.Vitality.medkits",
    ];
    let replicated = read(&mut agent, &w, Some(1), &fields);
    assert_eq!(
        replicated["e_fox.Vitality.hp"],
        json!(65),
        "relevant C# field: {replicated}"
    );
    assert_eq!(
        replicated["e_fox.Vitality.medkits"],
        json!(2),
        "another owner's field stays local: {replicated}"
    );
    assert_eq!(
        replicated["e_player02.Vitality.medkits"],
        json!(1),
        "owner C# field: {replicated}"
    );
    println!("CSHARP-REPLICATION {replicated}");

    // Captures: each process renders its own view (terrain, foliage, VFX composited).
    let host_png = captures.join("valley-host.png");
    let client_png = captures.join("valley-client.png");
    let t = Instant::now();
    agent.play(
        "capture",
        json!({"world": w, "path": host_png.to_string_lossy(), "marks": 12, "width": 1280, "height": 720}),
    );
    let host_capture_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    agent.play(
        "capture",
        json!({"world": w, "player": 1, "path": client_png.to_string_lossy(), "marks": 12, "width": 1280, "height": 720}),
    );
    let client_capture_ms = t.elapsed().as_secs_f64() * 1000.0;
    // What a windowed host frame draws (terrain chunks as meshes).
    let presented_png = captures.join("valley-presented.png");
    agent.play(
        "capture",
        json!({"world": w, "path": presented_png.to_string_lossy(), "marks": 0, "presented": true}),
    );
    assert!(host_png.exists() && client_png.exists() && presented_png.exists());

    // Sound: crackle and music through acoustics, with the streamed cells' probes.
    let wav = captures.join("valley.wav");
    let t = Instant::now();
    let audio = agent.play(
        "captureAudio",
        json!({"world": w, "path": wav.to_string_lossy(), "seconds": 2.0}),
    );
    let audio_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(audio["voices"], json!(["e_fire"]), "{audio}");
    assert!(audio["probes"].as_u64().unwrap() > 0, "{audio}");
    let acoustic_cells = audio["acousticCells"].as_array().unwrap();
    assert!(acoustic_cells.contains(&json!("c_10")), "{audio}");
    assert_eq!(audio["analysis"]["silent"], json!(false), "{audio}");

    let host_net = status(&mut agent, 0)["net"].clone();
    let client_net = status(&mut agent, 1)["net"].clone();
    let seconds = host_net["tick"].as_f64().unwrap().max(1.0) / 60.0;
    agent.play("stop", json!({"world": w}));
    println!(
        "VALLEY {}",
        serde_json::to_string_pretty(&json!({
            "startMs": start_ms,
            "host": host_started,
            "clientJoinMs": client_net["joinMs"],
            "downBytesPerSecond": host_net["downBytes"].as_f64().unwrap_or(0.0) / seconds,
            "upBytesPerSecond": host_net["upBytes"].as_f64().unwrap_or(0.0) / seconds,
            "replicatedEntities": client_net["replicated"],
            "prediction": {"corrections": client_net["corrections"], "rollbacks": client_net["rollbacks"]},
            "cells": cells,
            "fox": fox,
            "guard": guard,
            "captureMs": {"host": host_capture_ms, "client": client_capture_ms},
            "audio": {"captureMs": audio_ms, "probes": audio["probes"], "lufs": audio["analysis"]["integratedLufs"],
                "fire": audio["sources"][0]},
        }))
        .unwrap()
    );
}

/// The Valley in an SDL3 window (local play, live world streaming, terrain chunks
/// drawn as meshes, live audio with acoustics on the frame's worker): the Fox runs
/// east into the next cell; frame times come from heartbeats and a presented capture
/// shows what the window draws. Ignored by default because it opens a window:
/// `cargo test -p ara-player --test valley -- --ignored --nocapture`.
#[test]
#[ignore = "opens a window; run explicitly with --ignored"]
fn valley_plays_in_a_window() {
    let dir = demo_copy("valley-window");
    let captures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-polish/captures");
    std::fs::create_dir_all(&captures).unwrap();
    let mut agent = Agent::open(&dir, 9, |config| {
        config.pool.windowed = 1;
        config.pool.headless = 0;
    });
    agent.until("a warm window", |a| {
        a.play("status", json!({}))["pool"]["windowed"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    });
    let t = Instant::now();
    let world = agent.play("start", json!({"scene": SCENE, "window": true, "seed": 9}));
    let boot_ms = t.elapsed().as_secs_f64() * 1000.0;
    let id = world["world"].clone();
    agent.play(
        "input",
        json!({"world": id, "inputs": [
            {"action": "Move", "value": [0, 1], "holdTicks": 420},
            {"action": "Sprint", "value": true, "holdTicks": 420}
        ]}),
    );
    let started = Instant::now();
    let mut samples = Vec::new();
    while started.elapsed() < std::time::Duration::from_secs(8) {
        agent.host.tick();
        std::thread::sleep(std::time::Duration::from_millis(250));
        let s = agent.play("status", json!({}));
        if let Some(w) = s["worlds"].as_array().and_then(|w| w.first()) {
            samples.push(w["players"][0]["status"].clone());
        }
    }
    let last = samples.last().cloned().unwrap_or(Value::Null);
    assert!(last["fault"].is_null(), "{last}");
    assert_eq!(last["audio"], json!(true), "live audio: {last}");
    let cells = last["cells"].as_array().cloned().unwrap_or_default();
    assert!(cells.contains(&json!("c_10")), "streamed live: {last}");
    let read = read(
        &mut agent,
        &id,
        None,
        &["e_fox.Transform.position", "e_fox.CharacterMotion.grounded"],
    );
    assert!(
        position(&read, "e_fox.Transform.position")[0] > 33.0,
        "{read}"
    );
    assert_eq!(
        read["e_fox.CharacterMotion.grounded"],
        json!(true),
        "{read}"
    );
    let png = captures.join("valley-window.png");
    agent.play(
        "capture",
        json!({"world": id, "path": png.to_string_lossy(), "marks": 0, "presented": true}),
    );
    agent.play("stop", json!({"world": id}));
    let fps: Vec<f64> = samples
        .iter()
        .filter_map(|s| s["fps"].as_f64())
        .filter(|f| *f > 0.0)
        .collect();
    let p95: Vec<f64> = samples
        .iter()
        .filter_map(|s| s["frameMsP95"].as_f64())
        .filter(|f| *f > 0.0)
        .collect();
    println!(
        "VALLEY-WINDOW {}",
        serde_json::to_string_pretty(&json!({
            "bootFromPoolMs": boot_ms,
            "started": world["players"][0]["started"],
            "fps": fps,
            "frameMsP95": p95,
            "cells": cells,
            "fox": read,
        }))
        .unwrap()
    );
}

/// A settled stationary workload for comparable frame-budget measurements. Set
/// ARA_BENCH_PLAYER to a saved player binary to measure the same test before a patch.
#[test]
#[ignore = "opens a window; run explicitly for frame-budget evidence"]
fn valley_window_budget() {
    let dir = demo_copy("valley-budget");
    let output = repo().join("target/runtime-polish");
    let mut agent = Agent::open(&dir, 9, |config| {
        config.pool.windowed = 1;
        config.pool.headless = 0;
    });
    agent.until("a warm window", |a| {
        a.play("status", json!({}))["pool"]["windowed"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    });
    // View across the campfire at (16,0,16), with foliage behind it.
    agent.tool("ara_apply",json!({"doc":SCENE,"why":"Frame-budget camera","commands":[
        {"cmd":"property.set","args":{"entity":"e_camera","path":"FollowCamera.yaw","value":-60}},
        {"cmd":"property.set","args":{"entity":"e_camera","path":"FollowCamera.distance","value":9}}
    ]}));
    let started = agent.play("start", json!({"scene":SCENE,"window":true,"seed":9}));
    let world = started["world"].clone();
    let begin = Instant::now();
    let mut samples = Vec::new();
    while begin.elapsed() < std::time::Duration::from_secs(10) {
        agent.host.tick();
        std::thread::sleep(std::time::Duration::from_millis(250));
        let current = status(&mut agent, 0);
        assert!(current["fault"].is_null(), "{current}");
        if begin.elapsed() > std::time::Duration::from_secs(3) {
            samples.push(current);
        }
    }
    let label = if std::env::var_os("ARA_BENCH_PLAYER").is_some() {
        "before"
    } else {
        "after"
    };
    let path = output.join(format!("valley-camp-{label}.png"));
    agent.play(
        "capture",
        json!({"world":world,"path":path,"presented":true,"marks":0}),
    );
    agent.play("capture",json!({"world":world,"path":output.join(format!("valley-camp-reference-{label}.png")),"marks":0}));
    agent.play("stop", json!({"world":world}));
    let mut p95: Vec<f64> = samples
        .iter()
        .filter_map(|s| s["frameMsP95"].as_f64())
        .filter(|v| *v > 0.0)
        .collect();
    p95.sort_by(f64::total_cmp);
    assert!(!p95.is_empty(), "window did not report any frame timing");
    let budget_ms = 1000.0 / 60.0;
    assert!(
        p95.last().unwrap() <= &budget_ms,
        "60 Hz budget exceeded: {p95:?}"
    );
    let measurement = json!({"label":label,"budgetMs":budget_ms,"frameMsP95Windows":p95,"medianWindowP95":p95[p95.len()/2],"maxWindowP95":p95.last(),"samples":samples});
    std::fs::write(
        output.join(format!("valley-budget-{label}.json")),
        serde_json::to_vec_pretty(&measurement).unwrap(),
    )
    .unwrap();
    println!(
        "VALLEY-BUDGET {}",
        json!({"label":label,"medianWindowP95":measurement["medianWindowP95"],"maxWindowP95":measurement["maxWindowP95"]})
    );
}
