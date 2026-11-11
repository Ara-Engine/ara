#![allow(clippy::unwrap_used)] // Tests assert each step at its call site.
//! A studio started for an agent (`--background`) on a project with a failed import opens
//! its views by itself once the imports settle, so a `viewport` capture is served; the
//! import gate must never wait for a person who is not there.
use ara_proto::Endpoint;
use ara_rpc::Client;
use serde_json::json;
mod common;
use std::time::{Duration, Instant};

#[test]
#[ignore = "opens an unfocused window and needs a GPU: cargo test -p ara-editor --test background_capture -- --ignored"]
fn a_background_studio_on_a_failed_import_serves_a_viewport_capture() {
    let root = common::project("background-capture");
    let log = root.join("editor.err.log");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&root)
        .arg("--background")
        .stdout(std::fs::File::create(root.join("editor.out.log")).unwrap())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let result = std::panic::catch_unwind(|| capture(&root));
    let _ = child.kill();
    let _ = child.wait();
    let stderr = std::fs::read_to_string(&log).unwrap_or_default();
    if let Err(panic) = result {
        eprintln!("editor log:\n{stderr}");
        std::panic::resume_unwind(panic);
    }
    assert!(
        stderr.contains("imports: opened (no one at the window), 1 failed"),
        "the gate said how it opened:\n{stderr}"
    );
    assert!(
        root.join("scene.png").is_file(),
        "the capture wrote its picture"
    );
}

fn capture(root: &std::path::Path) {
    let started = Instant::now();
    let mut client = loop {
        if let Some(client) = Client::for_project(root) {
            break client;
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "no protocol server"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    let session = client
        .call(
            "session.begin",
            json!({"client": "test", "author": "human", "label": "background capture"}),
        )
        .unwrap();
    let handle = session["session"].clone();
    // The failed import is reported over the protocol, with its error.
    let started = Instant::now();
    let imports = loop {
        let said = client
            .call("editor.imports", json!({"session": handle}))
            .unwrap();
        if said["stage"] == "review" && said["gate"] == "open" {
            break said;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "never settled: {said}"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(
        imports["failed"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("models/crate.gltf")
    );
    assert!(
        !imports["failed"][0]["error"]
            .as_str()
            .unwrap()
            .trim()
            .is_empty()
    );
    // A viewport capture is served: the Scene is presented without anyone pressing Open.
    let started = Instant::now();
    let status = loop {
        let job = client
            .call(
                "capture",
                json!({"session": handle, "target": "viewport", "path": "scene.png"}),
            )
            .unwrap();
        let id = job["job"].as_str().unwrap().to_owned();
        let status = loop {
            let status = client
                .call("jobs.status", json!({"session": handle, "job": id}))
                .unwrap();
            if status["status"] != "running" {
                break status;
            }
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "capture never finished"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        // Until the first frame is up the Scene is not shown; the gate must not be why.
        if status["status"] == "done" || !status.to_string().contains("not shown") {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "never presented: {status}"
        );
        std::thread::sleep(Duration::from_millis(300));
    };
    assert_eq!(status["status"], "done", "{status}");
}
