#![allow(clippy::unwrap_used)]
//! SCRATCH (never committed): drives a running studio through the protocol: orbit, select,
//! Play and Stop, with `ARA_STUDIO_TRACE` on, so the loop's own log shows the Scene's cost.
use ara_proto::Endpoint;
use ara_rpc::Client;
use serde_json::{Value, json};
use std::process::Command;
use std::time::{Duration, Instant};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name}"))
}

#[test]
#[ignore = "scratch"]
fn live_loop() {
    let root = std::path::PathBuf::from(env("LIVE_PROJECT"));
    let log = std::path::PathBuf::from(env("LIVE_LOG"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&root)
        .args(["--frames", "1000000", "--background", "--size", "1600x900"])
        .env("ARA_STUDIO_TRACE", "1")
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let mut client = loop {
        if let Some(client) = Client::for_project(&root) {
            break client;
        }
        assert!(started.elapsed() < Duration::from_secs(300), "no server");
        std::thread::sleep(Duration::from_millis(200));
    };
    let session = client
        .call(
            "session.begin",
            json!({"client": "live", "author": "human", "label": "live loop"}),
        )
        .unwrap()["session"]
        .clone();
    let mark = |what: &str| eprintln!("LIVE {what} at {:?}", started.elapsed());
    let call = |client: &mut Client, method: &str, mut params: Value| {
        params["session"] = session.clone();
        let at = Instant::now();
        let out = client.call(method, params);
        (at.elapsed(), out)
    };
    std::thread::sleep(Duration::from_secs(20));
    let target: Vec<f64> = env("LIVE_TARGET")
        .split(',')
        .map(|v| v.parse().unwrap())
        .collect();
    let distance: f64 = env("LIVE_DISTANCE").parse().unwrap();
    // Orbit: 15 s at about 60 camera updates a second.
    mark("orbit-start");
    let t0 = Instant::now();
    let mut worst = Duration::ZERO;
    let mut calls = 0u32;
    while t0.elapsed() < Duration::from_secs(15) {
        let yaw = t0.elapsed().as_secs_f64() * 0.8;
        let (took, out) = call(
            &mut client,
            "editor.camera",
            json!({"target": target, "yaw": yaw, "pitch": 0.35, "distance": distance}),
        );
        out.unwrap();
        worst = worst.max(took);
        calls += 1;
        std::thread::sleep(Duration::from_millis(12));
    }
    mark("orbit-end");
    eprintln!("LIVE orbit: {calls} camera calls, worst round trip {worst:?}");
    std::thread::sleep(Duration::from_secs(3));
    // Select: alternate two entities.
    mark("select-start");
    let entities: Vec<String> = env("LIVE_ENTITIES").split(',').map(str::to_owned).collect();
    let mut worst = Duration::ZERO;
    for n in 0..24 {
        let (took, out) = call(
            &mut client,
            "editor.select",
            json!({"entities": [entities[n % entities.len()]]}),
        );
        out.unwrap();
        worst = worst.max(took);
        std::thread::sleep(Duration::from_millis(250));
    }
    mark("select-end");
    eprintln!("LIVE select: worst round trip {worst:?}");
    // Play and Stop.
    mark("play-start");
    let (took, out) = call(&mut client, "editor.command", json!({"id": "play.start"}));
    eprintln!("LIVE play.start {took:?}: {out:?}");
    std::thread::sleep(Duration::from_secs(12));
    mark("stop-start");
    let (took, out) = call(&mut client, "editor.command", json!({"id": "play.stop"}));
    eprintln!("LIVE play.stop {took:?}: {out:?}");
    std::thread::sleep(Duration::from_secs(6));
    mark("end");
    let _ = child.kill();
    let _ = child.wait();
}
