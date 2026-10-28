#![allow(clippy::unwrap_used)]
//! Agent-driven verification over MCP, the way an agent would play the demo:
//! start play from a warm pool → inject `Move` for 120 ticks → assert the Fox moved
//! and is grounded → labelled capture → patch latency while running → pause → patch a
//! light colour → keep changes → the scene document updated. Then determinism
//! (`test.run` twice in separate processes), Host + 2 launches, replay verification and
//! crash isolation (panic, access violation, hang → restart). Prints measured numbers.
mod support;

use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use support::{Agent, demo_copy};

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() as f64 * p).ceil() as usize).clamp(1, samples.len()) - 1]
}

#[test]
fn one_shot_run_and_capture_work_headless_over_mcp() {
    let dir = support::repo()
        .join("target/preview4-tests")
        .join(format!("headless-automation-{}", std::process::id()));
    support::copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("demo"), &dir);
    let mut agent = Agent::open(&dir, 440, |config| {
        config.pool.windowed = 0;
        config.pool.headless = 0;
    });
    let first = agent.tool("ara_run", json!({"ticks":12,"seed":7}));
    assert_eq!(first["tick"], 12, "{first}");
    assert!(
        first["logs"].is_array() && first["diagnostics"].is_array(),
        "{first}"
    );
    assert_eq!(agent.play("status", json!({}))["worlds"], json!([]));
    let second = agent.tool("ara_run", json!({"duration":0.2,"seed":7}));
    assert_eq!(second["tick"], 12, "{second}");
    assert_eq!(first["hash"], second["hash"]);
    let image = dir.join(".ara/automation.png");
    let captured = agent.tool("ara_capture", json!({"camera":"Camera","path":image,"width":96,"height":64,"preset":"low","marks":0,"seed":7}));
    assert_eq!(captured["tick"], 0, "{captured}");
    let pixels = image::open(&image).unwrap().to_rgba8();
    assert_eq!(pixels.dimensions(), (96, 64));
    let first_pixel = pixels.get_pixel(0, 0);
    assert!(
        pixels.pixels().any(|p| p != first_pixel),
        "capture must contain rendered scene content"
    );
    let bad = agent.raw("ara_capture", json!({"camera":"no such camera","path":dir.join(".ara/bad.png"),"width":64,"height":64,"preset":"low"}));
    assert_eq!(bad["isError"], true, "{bad}");
    assert_eq!(
        agent.play("status", json!({}))["worlds"],
        json!([]),
        "failed captures also release their world"
    );
    let tested = agent.tool(
        "ara_test",
        json!({"steps":[{"do":"step","ticks":2},{"do":"hash"}],"determinism":true}),
    );
    assert_eq!(tested["passed"], true, "{tested}");
    assert_eq!(tested["deterministic"], true, "{tested}");
}

#[cfg(windows)]
#[test]
#[ignore = "opens a player window and posts its native close event"]
fn closing_the_player_window_stops_the_world_cleanly() {
    let dir = demo_copy("close-window");
    let mut agent = Agent::open(&dir, 9, |config| {
        config.pool.headless = 0;
        config.pool.windowed = 1;
    });
    agent.until("a warm window", |a| {
        a.play("status", json!({}))["pool"]["windowed"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    });
    let world = agent.play("start", json!({"window":true}));
    let pid = world["players"][0]["pid"].as_u64().unwrap() as u32;
    agent.until("player frames", |a| {
        a.play("status", json!({}))["worlds"][0]["players"][0]["status"]["fps"]
            .as_f64()
            .unwrap_or(0.0)
            > 0.0
    });
    assert!(support::close_window(pid), "post WM_CLOSE to player {pid}");
    agent.until("a clean stopped world", |a| {
        a.play("status", json!({}))["worlds"]
            .as_array()
            .is_some_and(Vec::is_empty)
    });
    println!("PLAYER-CLOSE clean pid={pid}");
}

#[test]
fn agent_plays_patches_keeps_and_verifies_the_demo_over_mcp() {
    let dir = demo_copy("scenario");
    let captures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-polish/captures");
    // An importer upgrade must not fail Play: record an older glTF importer version.
    let sidecar = dir
        .join(ara_doc::AUTHORING_ROOT)
        .join("assets/fox/Fox.glb.import.json");
    let current = std::fs::read_to_string(&sidecar).unwrap();
    let recorded =
        serde_json::from_str::<Value>(&current).unwrap()["records"]["i_settings"]["version"]
            .as_str()
            .unwrap()
            .to_owned();
    // The upgrade target is the installed importer, not whatever the demo sidecar recorded.
    let installed = ara_importers::registry()
        .unwrap()
        .get("ara.gltf")
        .unwrap()
        .version()
        .to_owned();
    std::fs::write(&sidecar, current.replace(&recorded, "2-gltf14-legacy")).unwrap();
    let mut agent = Agent::open(&dir, 7, |_| {});

    // Warm the pool; a first Play also cooks the Fox (out of process) into the cache.
    agent.pool_ready();
    let first = Instant::now();
    let warm = agent.play("start", json!({"startPaused": true}));
    let first_play_ms = first.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(warm["notes"][0]["code"], json!("ARA7306"), "{warm}");
    assert!(
        std::fs::read_to_string(&sidecar)
            .unwrap()
            .contains(&installed)
    );
    agent.play("stop", json!({"world": warm["world"]}));
    agent.pool_ready();

    // 1. Play from the pool: snapshot → shared memory → player → tick 1.
    let start = Instant::now();
    let world = agent.play("start", json!({"seed": 7}));
    let boot_ms = start.elapsed().as_secs_f64() * 1000.0;
    let id = world["world"].clone();
    // Shared-memory sections are Windows-only (`ara_play::shm`); elsewhere the
    // snapshot travels inline.
    assert_eq!(
        world["transport"],
        json!(if cfg!(windows) { "shm" } else { "inline" })
    );
    assert!(
        boot_ms < 1000.0,
        "Play → first tick from the pool took {boot_ms:.1} ms"
    );

    // 2–3. Move for 120 ticks; the Fox walked forward and stands on the ground.
    let before = agent.play(
        "read",
        json!({"world": id, "paths": ["e_fox.Transform.position.z"]}),
    );
    let stepped = agent.play(
        "step",
        json!({"world": id, "ticks": 120,
        "inputs": [{"action": "Move", "value": [0, 1], "holdTicks": 120}]}),
    );
    let after = agent.play("read", json!({"world": id,
        "paths": ["e_fox.Transform.position.z", "e_fox.CharacterMotion.grounded", "e_bot1.Wanderer.goals"]}));
    let z0 = before["values"]["e_fox.Transform.position.z"]
        .as_f64()
        .unwrap();
    let z1 = after["values"]["e_fox.Transform.position.z"]
        .as_f64()
        .unwrap();
    assert!(z1 - z0 > 1.0, "the Fox moved {:.2} m", z1 - z0);
    assert_eq!(
        after["values"]["e_fox.CharacterMotion.grounded"],
        json!(true)
    );

    // 4. Labelled capture.
    let shot = agent.play(
        "capture",
        json!({"world": id, "path": captures.join("scenario-walked.png").to_string_lossy()}),
    );
    let marks = shot["marks"].as_array().unwrap();
    assert!(
        marks
            .iter()
            .any(|m| m["label"].as_str().unwrap().contains("Fox (e_fox)")),
        "{marks:?}"
    );

    // Live patch round trips while running (applied at the next tick, acked after it).
    let mut patch_ms = Vec::new();
    for i in 0..20 {
        let t = Instant::now();
        let r = agent.play(
            "patch",
            json!({"world": id,
            "sets": [{"entity": "e_lamp2", "path": "Light.intensity", "value": 2000 + i * 25}]}),
        );
        patch_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(r["readBack"][0].as_f64(), Some(f64::from(2000 + i * 25)));
    }
    let patch_p95 = percentile(&mut patch_ms.clone(), 0.95);
    assert!(patch_p95 < 50.0, "live patch p95 {patch_p95:.1} ms");

    // 5–7. Pause, patch a light colour, keep the change.
    agent.play("pause", json!({"world": id}));
    let colour = json!([0.2, 0.4, 1.0, 1.0]);
    let patched = agent.play("patch", json!({"world": id, "sets": [{"entity": "e_lamp1", "path": "Light.color", "value": colour}]}));
    assert_eq!(patched["readBack"][0], colour);
    let kept = agent.play(
        "keep",
        json!({"world": id, "paths": ["Light.color"], "why": "Bluer lamp from play"}),
    );
    assert_eq!(kept["kept"].as_array().unwrap().len(), 1, "{kept}");
    assert!(kept["transaction"]["txn"].is_string(), "{kept}");

    // 8. The edit world's scene document now holds the kept colour (unreviewed agent edit).
    let doc = agent
        .host
        .store()
        .workspace()
        .docs()
        .find(|(_, d)| d.id.as_str() == "sc_outpost1")
        .unwrap()
        .1
        .clone();
    assert_eq!(doc.record("e_lamp1").unwrap()["Light"]["color"], colour);
    let review = agent.tool(
        "ara_session",
        json!({"action": "status", "session": agent.session}),
    );
    let after_capture = agent.play(
        "capture",
        json!({"world": id, "path": captures.join("scenario-relit.png").to_string_lossy()}),
    );
    agent.play("stop", json!({"world": id}));

    // Determinism: the same seed and inputs in two separate players → equal hashes.
    agent.pool_ready();
    let test = agent.tool("ara_test", json!({"seed": 11, "determinism": true, "steps": [
        {"do": "step", "ticks": 120, "inputs": [{"action": "Move", "value": [0.4, 1], "holdTicks": 90}, {"action": "Jump", "value": true, "afterTicks": 30}]},
        {"do": "expect", "path": "e_fox.CharacterMotion.grounded", "compare": "eq", "value": true},
        {"do": "expect", "path": "e_fox.Transform.position.z", "compare": "gt", "value": 1.0},
        {"do": "until", "path": "e_bot2.Wanderer.goals", "compare": "ge", "value": 1, "maxTicks": 600}
    ]}));
    assert_eq!(test["passed"], json!(true), "{test}");
    assert_eq!(test["deterministic"], json!(true), "{test}");

    // Replay: record from tick 0, then verify every tick's hash.
    agent.pool_ready();
    let w = agent.play("start", json!({"startPaused": true, "seed": 3}))["world"].clone();
    agent.play("record", json!({"world": w}));
    agent.play("step", json!({"world": w, "ticks": 90, "inputs": [{"action": "Move", "value": [1, 1], "holdTicks": 60}]}));
    let file = dir.join("demo.replay").to_string_lossy().into_owned();
    agent.play("saveReplay", json!({"world": w, "path": file}));
    let replay = agent.play("replay", json!({"world": w, "path": file}));
    assert_eq!(replay["verified"], json!(true));
    agent.play("stop", json!({"world": w}));

    // Host + 2 clients: three processes, clients handshake with the server.
    agent.pool_ready();
    let t = Instant::now();
    let multi = agent.play(
        "start",
        json!({"mode": "host", "clients": 2, "startPaused": true}),
    );
    let multi_ms = t.elapsed().as_secs_f64() * 1000.0;
    let players = multi["players"].as_array().unwrap();
    assert_eq!(players.len(), 3);
    assert!(players[1]["started"]["netHandshakeMs"].is_number());
    agent.play("stop", json!({"world": multi["world"]}));

    // Crash isolation: each fault is reported structurally and the world restarts.
    let mut crashes = Vec::new();
    for kind in ["panic", "access", "hang"] {
        let dump_dir = dir.join(".ara/play/crashes");
        let old_dumps: std::collections::BTreeSet<_> = std::fs::read_dir(&dump_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        agent.pool_ready();
        let w = agent.play("start", json!({"startPaused": true}))["world"].clone();
        let t = Instant::now();
        let crashed = agent.raw(
            "ara_play",
            json!({"action": "crash", "world": w, "kind": kind}),
        );
        let detected_ms = t.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(crashed["isError"], json!(true), "{kind}");
        let status = agent.play("status", json!({}));
        let fault = status["worlds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["world"] == w)
            .unwrap()["fault"]
            .clone();
        let expected = match kind {
            "panic" => "player_panicked",
            "access" => "player_crashed",
            _ => "player_unresponsive",
        };
        assert_eq!(fault["code"], json!(expected), "{fault}");
        if kind != "hang" {
            let dumps: Vec<_> = std::fs::read_dir(&dump_dir)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "dmp") && !old_dumps.contains(p))
                .collect();
            assert!(
                !dumps.is_empty(),
                "{kind} must produce a minidump in {}",
                dump_dir.display()
            );
            let output = support::repo().join("target/runtime-polish/crashes");
            std::fs::create_dir_all(&output).unwrap();
            for dump in dumps {
                use std::io::Read;
                let mut signature = [0; 4];
                std::fs::File::open(&dump)
                    .unwrap()
                    .read_exact(&mut signature)
                    .unwrap();
                assert_eq!(&signature, b"MDMP");
                let target = output.join(format!(
                    "{kind}-{}",
                    dump.file_name().unwrap().to_string_lossy()
                ));
                std::fs::copy(&dump, &target).unwrap();
                let report = dump.with_extension("crash.json");
                if report.exists() {
                    std::fs::copy(report, target.with_extension("crash.json")).unwrap();
                }
                println!("PLAYER-DUMP {}", target.display());
            }
        }
        agent.pool_ready();
        let t = Instant::now();
        agent.play("restart", json!({"world": w}));
        let restart_ms = t.elapsed().as_secs_f64() * 1000.0;
        agent.play("step", json!({"world": w, "ticks": 5}));
        agent.play("stop", json!({"world": w}));
        crashes.push(json!({"kind": kind, "code": expected, "exit": fault["exitCode"], "detectedMs": detected_ms, "restartMs": restart_ms}));
    }

    let summary = json!({
        "firstPlayIncludingImportMs": first_play_ms,
        "bootFromPoolMs": boot_ms,
        "ownerPrepareMs": world["prepareMs"],
        "ownerLaunchMs": world["bootMs"],
        "playerStartedMs": world["players"][0]["started"],
        "snapshotBytes": world["snapshotBytes"],
        "step120": {"tick": stepped["tick"], "foxMovedM": z1 - z0},
        "patchRoundTripMs": {"p50": percentile(&mut patch_ms.clone(), 0.5), "p95": patch_p95, "max": percentile(&mut patch_ms, 1.0)},
        "captureMarks": marks.len(),
        "keptTransaction": kept["transaction"]["txn"],
        "unreviewedAgentTransactions": review["unreviewed"],
        "relitCaptureHash": after_capture["hash"],
        "determinismHash": test["runs"][0]["hash"],
        "hostPlus2Ms": multi_ms,
        "crashes": crashes,
    });
    println!(
        "SCENARIO {}",
        serde_json::to_string_pretty(&summary).unwrap()
    );
    drop(agent);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same demo in an SDL3 window, driven over MCP: a warm windowed player from the
/// pool, real-time play with injected movement, frame times from heartbeats and a
/// labelled capture. Ignored by default because it opens a window:
/// `cargo test -p ara-player --test scenario -- --ignored --nocapture`.
#[test]
#[ignore = "opens a window; run explicitly with --ignored"]
fn agent_plays_the_demo_in_a_window() {
    let dir = demo_copy("window");
    let captures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-polish/captures");
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
    let world = agent.play("start", json!({"window": true, "seed": 9}));
    let boot_ms = t.elapsed().as_secs_f64() * 1000.0;
    let id = world["world"].clone();
    agent.play(
        "input",
        json!({"world": id, "inputs": [
            {"action": "Move", "value": [0.3, 1], "holdTicks": 150},
            {"action": "Sprint", "value": true, "holdTicks": 90, "afterTicks": 60}
        ]}),
    );
    let started = Instant::now();
    let mut status = Value::Null;
    while started.elapsed() < Duration::from_secs(4) {
        agent.host.tick();
        std::thread::sleep(Duration::from_millis(20));
        let s = agent.play("status", json!({}));
        if let Some(w) = s["worlds"].as_array().and_then(|w| w.first()) {
            status = w["players"][0]["status"].clone();
        }
    }
    let read = agent.play("read", json!({"world": id, "paths": ["e_fox.Transform.position.z", "e_fox.CharacterMotion.grounded"]}));
    assert!(
        read["values"]["e_fox.Transform.position.z"]
            .as_f64()
            .unwrap()
            > 1.0,
        "{read}"
    );
    let shot = agent.play(
        "capture",
        json!({"world": id, "path": captures.join("window.png").to_string_lossy()}),
    );
    assert!(!shot["marks"].as_array().unwrap().is_empty());
    agent.play("stop", json!({"world": id}));
    println!(
        "WINDOW {}",
        serde_json::to_string_pretty(&json!({
            "bootFromPoolMs": boot_ms,
            "started": world["players"][0]["started"],
            "status": status,
            "tick": read["tick"],
            "fox": read["values"],
        }))
        .unwrap()
    );
    drop(agent);
    let _ = std::fs::remove_dir_all(&dir);
}
