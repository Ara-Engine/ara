#![allow(clippy::unwrap_used)]
//! Agent-driven verification over MCP, the way an agent would play the demo:
//! start play from a warm pool → inject `Move` for 120 ticks → assert the Fox moved
//! and is grounded → labelled capture → patch latency while running → pause → patch a
//! light colour → keep changes → the scene document updated. Then determinism
//! (`test.run` twice in separate processes), Host + 2 launches, replay verification and
//! crash isolation (panic, access violation, hang → restart). Prints measured numbers.
use ara_host::{Host, HostConfig, Project};
use ara_mcp::Adapter;
use ara_play_host::{PlayConfig, PlayService};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let name = entry.file_name();
        if name == ".ara" {
            continue;
        }
        if entry.path().is_dir() {
            copy(&entry.path(), &to.join(name));
        } else {
            std::fs::copy(entry.path(), to.join(name)).unwrap();
        }
    }
}

/// The import worker next to the player binary, built on demand.
fn import_worker(player: &Path) -> PathBuf {
    let worker = player.with_file_name(format!("ara-import-worker{}", std::env::consts::EXE_SUFFIX));
    if !worker.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "ara-import-worker"])
            .env("CARGO_BUILD_JOBS", "2")
            .status()
            .unwrap();
        assert!(status.success(), "building ara-import-worker failed");
    }
    worker
}

struct Agent {
    host: Host,
    mcp: Adapter,
    session: String,
}

impl Agent {
    fn tool(&mut self, name: &str, args: Value) -> Value {
        let result = self.raw(name, args);
        assert_eq!(result["isError"], json!(false), "{name}: {}", result["content"]);
        result["structuredContent"].clone()
    }
    fn raw(&mut self, name: &str, mut args: Value) -> Value {
        if name != "ara_session" {
            args["session"] = json!(self.session);
        }
        self.mcp
            .call_tool(&mut self.host, name, args.as_object().cloned().unwrap())
            .unwrap()
    }
    fn play(&mut self, action: &str, args: Value) -> Value {
        let mut args = args;
        args["action"] = json!(action);
        self.tool("ara_play", args)
    }
    /// Lets the owner run its tick (pool refill, heartbeats, imports) until `ready`.
    fn until(&mut self, what: &str, mut ready: impl FnMut(&mut Self) -> bool) {
        let start = Instant::now();
        while !ready(self) {
            assert!(start.elapsed() < Duration::from_secs(120), "timed out waiting for {what}");
            self.host.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn pool_ready(&mut self) {
        self.until("a warm pool", |a| {
            let status = a.play("status", json!({}));
            status["pool"]["headless"].as_u64().unwrap_or(0) >= 2
        });
    }
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() as f64 * p).ceil() as usize).clamp(1, samples.len()) - 1]
}

#[test]
fn agent_plays_patches_keeps_and_verifies_the_demo_over_mcp() {
    let player = PathBuf::from(env!("CARGO_BIN_EXE_ara-player"));
    let worker = import_worker(&player);
    let dir = std::env::temp_dir().join(format!("ara-scenario-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/demo")), &dir);
    let captures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sn/scenario");

    let mut project = Project::open(&dir).unwrap();
    ara_play_host::register_engine_schemas(&mut project.db);
    let mut host = Host::open(project, HostConfig { seed: Some(7), ..Default::default() }).unwrap();
    let mut config = PlayConfig::for_project(&dir);
    config.pool.executable = player;
    config.import_worker = worker;
    host.register(Box::new(PlayService::new(&dir, config))).unwrap();
    let mut agent = Agent { host, mcp: Adapter::new(), session: String::new() };
    let begun = agent.tool("ara_session", json!({"action": "begin", "client": "scenario", "author": "agent:scenario"}));
    agent.session = begun["session"].as_str().unwrap().to_owned();

    // Warm the pool; a first Play also cooks the Fox (out of process) into the cache.
    agent.pool_ready();
    let first = Instant::now();
    let warm = agent.play("start", json!({"startPaused": true}));
    let first_play_ms = first.elapsed().as_secs_f64() * 1000.0;
    agent.play("stop", json!({"world": warm["world"]}));
    agent.pool_ready();

    // 1. Play from the pool: snapshot → shared memory → player → tick 1.
    let start = Instant::now();
    let world = agent.play("start", json!({"seed": 7}));
    let boot_ms = start.elapsed().as_secs_f64() * 1000.0;
    let id = world["world"].clone();
    assert_eq!(world["transport"], json!("shm"));
    assert!(boot_ms < 1000.0, "Play → first tick from the pool took {boot_ms:.1} ms");

    // 2–3. Move for 120 ticks; the Fox walked forward and stands on the ground.
    let before = agent.play("read", json!({"world": id, "paths": ["e_fox.Transform.position.z"]}));
    let stepped = agent.play("step", json!({"world": id, "ticks": 120,
        "inputs": [{"action": "Move", "value": [0, 1], "holdTicks": 120}]}));
    let after = agent.play("read", json!({"world": id,
        "paths": ["e_fox.Transform.position.z", "e_fox.CharacterMotion.grounded", "e_bot1.Wanderer.goals"]}));
    let z0 = before["values"]["e_fox.Transform.position.z"].as_f64().unwrap();
    let z1 = after["values"]["e_fox.Transform.position.z"].as_f64().unwrap();
    assert!(z1 - z0 > 1.0, "the Fox moved {:.2} m", z1 - z0);
    assert_eq!(after["values"]["e_fox.CharacterMotion.grounded"], json!(true));

    // 4. Labelled capture.
    let shot = agent.play("capture", json!({"world": id, "path": captures.join("scenario-walked.png").to_string_lossy()}));
    let marks = shot["marks"].as_array().unwrap();
    assert!(marks.iter().any(|m| m["label"].as_str().unwrap().contains("Fox (e_fox)")), "{marks:?}");

    // Live patch round trips while running (applied at the next tick, acked after it).
    let mut patch_ms = Vec::new();
    for i in 0..20 {
        let t = Instant::now();
        let r = agent.play("patch", json!({"world": id,
            "sets": [{"entity": "e_lamp2", "path": "Light.intensity", "value": 2000 + i * 25}]}));
        patch_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(r["readBack"][0], json!(2000 + i * 25));
    }
    let patch_p95 = percentile(&mut patch_ms.clone(), 0.95);
    assert!(patch_p95 < 50.0, "live patch p95 {patch_p95:.1} ms");

    // 5–7. Pause, patch a light colour, keep the change.
    agent.play("pause", json!({"world": id}));
    let colour = json!([0.2, 0.4, 1.0, 1.0]);
    let patched = agent.play("patch", json!({"world": id, "sets": [{"entity": "e_lamp1", "path": "Light.color", "value": colour}]}));
    assert_eq!(patched["readBack"][0], colour);
    let kept = agent.play("keep", json!({"world": id, "paths": ["Light.color"], "why": "Bluer lamp from play"}));
    assert_eq!(kept["kept"].as_array().unwrap().len(), 1, "{kept}");
    assert!(kept["transaction"]["txn"].is_string(), "{kept}");

    // 8. The edit world's scene document now holds the kept colour (unreviewed agent edit).
    let doc = agent.host.store().workspace().docs().find(|(_, d)| d.id.as_str() == "sc_outpost1").unwrap().1.clone();
    assert_eq!(doc.record("e_lamp1").unwrap()["Light"]["color"], colour);
    let review = agent.tool("ara_session", json!({"action": "status", "session": agent.session}));
    let after_capture = agent.play("capture", json!({"world": id, "path": captures.join("scenario-relit.png").to_string_lossy()}));
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
    let multi = agent.play("start", json!({"mode": "host", "clients": 2, "startPaused": true}));
    let multi_ms = t.elapsed().as_secs_f64() * 1000.0;
    let players = multi["players"].as_array().unwrap();
    assert_eq!(players.len(), 3);
    assert!(players[1]["started"]["netHandshakeMs"].is_number());
    agent.play("stop", json!({"world": multi["world"]}));

    // Crash isolation: each fault is reported structurally and the world restarts.
    let mut crashes = Vec::new();
    for kind in ["panic", "access", "hang"] {
        agent.pool_ready();
        let w = agent.play("start", json!({"startPaused": true}))["world"].clone();
        let t = Instant::now();
        let crashed = agent.raw("ara_play", json!({"action": "crash", "world": w, "kind": kind}));
        let detected_ms = t.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(crashed["isError"], json!(true), "{kind}");
        let status = agent.play("status", json!({}));
        let fault = status["worlds"].as_array().unwrap().iter().find(|x| x["world"] == w).unwrap()["fault"].clone();
        let expected = match kind {
            "panic" => "player_panicked",
            "access" => "player_crashed",
            _ => "player_unresponsive",
        };
        assert_eq!(fault["code"], json!(expected), "{fault}");
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
        "playerStartedMs": world["players"][0]["started"],
        "snapshotBytes": world["snapshotBytes"],
        "step120": {"tick": stepped["tick"], "foxMovedM": z1 - z0},
        "patchRoundTripMs": {"p50": percentile(&mut patch_ms.clone(), 0.5), "p95": patch_p95, "max": percentile(&mut patch_ms, 1.0)},
        "captureMarks": marks.len(),
        "keptTransaction": kept["transaction"]["txn"],
        "unreviewed": review,
        "relitCaptureHash": after_capture["hash"],
        "determinismHash": test["runs"][0]["hash"],
        "hostPlus2Ms": multi_ms,
        "crashes": crashes,
    });
    println!("SCENARIO {}", serde_json::to_string_pretty(&summary).unwrap());
    drop(agent);
    let _ = std::fs::remove_dir_all(&dir);
}
