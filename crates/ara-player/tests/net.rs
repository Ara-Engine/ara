#![allow(clippy::unwrap_used)]
//! Networked Play (D19) over MCP, every peer its own player process, real Lightyear
//! replication (not lockstep copies):
//!
//! - Host + 1 behind a simulated 40 ms link: the host drives player 1, the client
//!   drives its own avatar (owner prediction, rollback on correction); both worlds
//!   converge, each process captures its own view.
//! - Dedicated + 2: two clients move; each sees the other's avatar through remote
//!   interpolation.
//!
//! Prints join time, bandwidth and prediction error.
mod support;

use serde_json::{Value, json};
use std::{path::Path, time::Instant};
use support::{Agent, demo_copy};

/// The status of player `index` (0 = authority) of the latest world.
fn status(agent: &mut Agent, index: usize) -> Value {
    let status = agent.play("status", json!({}));
    status["worlds"][0]["players"][index]["status"].clone()
}

/// Waits until the authority reached `tick`.
fn until_tick(agent: &mut Agent, tick: u64) {
    agent.until("the authority's tick", |a| {
        status(a, 0)["tick"].as_u64().unwrap_or(0) >= tick
    });
}

fn position(read: &Value, path: &str) -> [f64; 3] {
    let p = &read["values"][path];
    [0, 1, 2].map(|i| p[i].as_f64().unwrap_or(f64::NAN))
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[test]
fn host_and_dedicated_replicate_predict_and_capture() {
    let dir = demo_copy("net");
    let captures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-polish/captures");
    std::fs::create_dir_all(&captures).unwrap();
    let mut agent = Agent::open(&dir, 21, |_| {});
    agent.pool_ready();

    // Host + 1 client behind a 40 ms (one-way), ±5 ms, 1% loss link.
    let t = Instant::now();
    let started = agent.play(
        "start",
        json!({"mode": "host", "clients": 1,
               "conditions": [{"latencyMs": 40, "jitterMs": 5, "loss": 100}]}),
    );
    let start_ms = t.elapsed().as_secs_f64() * 1000.0;
    let w = started["world"].clone();
    let handshake_ms = started["players"][1]["started"]["netHandshakeMs"].clone();
    assert!(handshake_ms.is_number(), "{started}");
    // Networked worlds run on the authority's timeline: no explicit steps.
    let step = agent.raw(
        "ara_play",
        json!({"action": "step", "world": w, "ticks": 1}),
    );
    assert_eq!(step["isError"], json!(true), "{step}");

    agent.until("the client's first authoritative tick", |a| {
        status(a, 1)["net"]["joinMs"].is_number()
    });
    let begin = status(&mut agent, 0)["tick"].as_u64().unwrap();
    let before = agent.play(
        "read",
        json!({"world": w, "paths": ["e_fox.Transform.position", "e_player02.Transform.position"]}),
    );
    // Player 1 walks +Z on the host; player 2 walks +X on the client (predicted).
    agent.play(
        "input",
        json!({"world": w, "inputs": [
            {"action": "Move", "value": [0, 1], "holdTicks": 90, "player": 1},
            {"action": "Move", "value": [1, 0], "holdTicks": 90, "player": 2}
        ]}),
    );
    // Mid-walk: the client's own avatar runs ahead of the authority (prediction).
    until_tick(&mut agent, begin + 45);
    let client_mid = agent.play(
        "read",
        json!({"world": w, "player": 1, "paths": ["e_player02.Transform.position"]}),
    );
    // After the walk and a settle period, both worlds agree.
    until_tick(&mut agent, begin + 90 + 60);
    let paths = json!(["e_fox.Transform.position", "e_player02.Transform.position"]);
    let host = agent.play("read", json!({"world": w, "paths": paths}));
    let client = agent.play("read", json!({"world": w, "player": 1, "paths": paths}));
    let fox = [
        position(&before, "e_fox.Transform.position"),
        position(&host, "e_fox.Transform.position"),
        position(&client, "e_fox.Transform.position"),
    ];
    let p2 = [
        position(&before, "e_player02.Transform.position"),
        position(&host, "e_player02.Transform.position"),
        position(&client, "e_player02.Transform.position"),
    ];
    assert!(fox[1][2] - fox[0][2] > 2.0, "host walked +Z: {fox:?}");
    assert!(
        p2[0][0] - p2[1][0] > 2.0,
        "client walked camera-right (-X): {p2:?}"
    );
    assert!(
        distance(fox[1], fox[2]) < 0.02,
        "host avatar replicated: {fox:?}"
    );
    assert!(
        distance(p2[1], p2[2]) < 0.02,
        "predicted avatar converged: {p2:?}"
    );
    let mid = position(&client_mid, "e_player02.Transform.position");
    assert!(mid[0] < p2[0][0], "client predicted its own walk: {mid:?}");

    // Facing replicates with the motor; each process's follow camera tracks its own
    // player (the client's camera sits behind player 2).
    let probe = json!([
        "e_player02.Transform.rotation",
        "e_camera.Transform.position"
    ]);
    let host_probe = agent.play("read", json!({"world": w, "paths": probe}));
    let client_probe = agent.play("read", json!({"world": w, "player": 1, "paths": probe}));
    for probe in [&host_probe, &client_probe] {
        let yaw = position(probe, "e_player02.Transform.rotation")[1];
        assert!(
            (yaw + 90.0).abs() < 0.5,
            "player 2 faces camera-right (-X): {probe}"
        );
    }
    let camera = position(&client_probe, "e_camera.Transform.position");
    assert!(
        (camera[0] - p2[2][0]).abs() < 0.05,
        "client camera follows player 2: {client_probe}"
    );
    let movement_begin = status(&mut agent, 0)["tick"].as_u64().unwrap();
    agent.play(
        "input",
        json!({"world":w,"inputs":[
        {"action":"Look","value":[36,0],"holdTicks":1,"player":2},
            {"action":"Move","value":[0,1],"holdTicks":60,"player":2},
            {"action":"Sprint","value":true,"holdTicks":60,"player":2},
            {"action":"Jump","value":true,"holdTicks":1,"player":2}
        ]}),
    );
    until_tick(&mut agent, movement_begin + 20);
    let airborne = agent.play("read",json!({"world":w,"player":1,"paths":["e_player02.Transform.position","e_camera.FollowCamera.yaw"]}));
    let jumped = position(&airborne, "e_player02.Transform.position");
    assert!(
        jumped[1] > p2[2][1] + 0.1,
        "client predicts jump height: {airborne}"
    );
    assert_eq!(
        airborne["values"]["e_camera.FollowCamera.yaw"].as_f64(),
        Some(90.0)
    );
    until_tick(&mut agent, movement_begin + 140);
    let moved_host=agent.play("read",json!({"world":w,"paths":["e_player02.Transform.position","e_player02.CharacterMotion.grounded"]}));
    let moved_client=agent.play("read",json!({"world":w,"player":1,"paths":["e_player02.Transform.position","e_player02.CharacterMotion.grounded"]}));
    let sprinted = position(&moved_host, "e_player02.Transform.position");
    assert!(
        sprinted[0] - p2[1][0] > 4.0,
        "camera-relative sprint turns forward to +X: {moved_host}"
    );
    assert!(
        distance(
            sprinted,
            position(&moved_client, "e_player02.Transform.position")
        ) < 0.02,
        "predicted movement converges: {moved_host} / {moved_client}"
    );
    assert_eq!(
        moved_client["values"]["e_player02.CharacterMotion.grounded"],
        json!(true)
    );
    println!(
        "PREDICTED-CONTROLS {}",
        json!({"airborne":airborne,"authority":moved_host,"client":moved_client})
    );
    let host_png = captures.join("net-host.png");
    let client_png = captures.join("net-client.png");
    let t = Instant::now();
    agent.play(
        "capture",
        json!({"world": w, "path": host_png.to_string_lossy(), "marks": 8}),
    );
    agent.play(
        "capture",
        json!({"world": w, "player": 1, "path": client_png.to_string_lossy(), "marks": 8}),
    );
    let capture_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(host_png.exists() && client_png.exists());
    let host_net = status(&mut agent, 0)["net"].clone();
    let client_net = status(&mut agent, 1)["net"].clone();
    let ticks = host_net["tick"].as_f64().unwrap().max(1.0);
    let seconds = ticks / 60.0;
    agent.play("stop", json!({"world": w}));

    // Dedicated + 2: each client sees the other's avatar move (interpolated).
    agent.pool_ready();
    let t = Instant::now();
    let dedicated = agent.play("start", json!({"mode": "dedicated", "clients": 2}));
    let dedicated_ms = t.elapsed().as_secs_f64() * 1000.0;
    let d = dedicated["world"].clone();
    agent.until("both clients joined", |a| {
        status(a, 1)["net"]["joinMs"].is_number() && status(a, 2)["net"]["joinMs"].is_number()
    });
    let begin = status(&mut agent, 0)["tick"].as_u64().unwrap();
    let paths = json!([
        "e_player02.Transform.position",
        "e_player03.Transform.position"
    ]);
    let before = agent.play("read", json!({"world": d, "paths": paths}));
    agent.play(
        "input",
        json!({"world": d, "inputs": [
            {"action": "Move", "value": [0, 1], "holdTicks": 60, "player": 2},
            {"action": "Move", "value": [0, -1], "holdTicks": 60, "player": 3}
        ]}),
    );
    until_tick(&mut agent, begin + 60 + 60);
    let server = agent.play("read", json!({"world": d, "paths": paths}));
    let seen_by_2 = agent.play("read", json!({"world": d, "player": 1, "paths": paths}));
    let seen_by_3 = agent.play("read", json!({"world": d, "player": 2, "paths": paths}));
    let p3 = "e_player03.Transform.position";
    let p2 = "e_player02.Transform.position";
    assert!(
        position(&server, p2)[2] - position(&before, p2)[2] > 1.0,
        "{server}"
    );
    assert!(
        position(&before, p3)[2] - position(&server, p3)[2] > 1.0,
        "{server}"
    );
    assert!(distance(position(&server, p3), position(&seen_by_2, p3)) < 0.02);
    assert!(distance(position(&server, p2), position(&seen_by_3, p2)) < 0.02);
    let remote = status(&mut agent, 1)["net"].clone();
    agent.play("stop", json!({"world": d}));

    println!(
        "NET {}",
        serde_json::to_string_pretty(&json!({
            "hostPlusOne": {
                "startMs": start_ms,
                "clientHandshakeMs": handshake_ms,
                "clientJoinMs": client_net["joinMs"],
                "hostBeginMs": host_net["beginMs"],
                "seconds": seconds,
                "downBytesPerSecond": host_net["downBytes"].as_f64().unwrap() / seconds,
                "upBytesPerSecond": host_net["upBytes"].as_f64().unwrap() / seconds,
                "fieldPayloadBytes": host_net["payloadBytes"],
                "dropped": host_net["dropped"],
                "prediction": {
                    "checkpoints": client_net["checkpoints"],
                    "corrections": client_net["corrections"],
                    "meanErrorMm": client_net["correctionMeanMm"],
                    "maxErrorMm": client_net["correctionMaxMm"],
                    "rollbacks": client_net["rollbacks"],
                    "replayedTicks": client_net["replayedTicks"],
                },
                "replicatedEntities": client_net["replicated"],
                "capturesMs": capture_ms,
            },
            "dedicatedPlusTwo": {
                "startMs": dedicated_ms,
                "interpolated": remote["interpolated"],
                "predicted": remote["predicted"],
                "joinMs": remote["joinMs"],
            },
        }))
        .unwrap()
    );
}
