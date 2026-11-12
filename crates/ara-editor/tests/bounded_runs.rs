#![allow(clippy::unwrap_used)] // Tests assert each step at its call site.
//! `--frames N`, `--screenshot` and the long hidden run, the way the release smoke steps and
//! `ara-samples editor` start the editor: each on a project with a failed import, so the
//! import gate (unattended for these) is part of what ends or serves them.
mod common;

use ara_proto::Endpoint;
use ara_rpc::Client;
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

fn spawn(root: &Path, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(root)
        .args(args)
        .stdout(std::fs::File::create(root.join("editor.out.log")).unwrap())
        .stderr(std::fs::File::create(root.join("editor.err.log")).unwrap())
        .spawn()
        .unwrap()
}

/// Waits for `child` to end by itself; kills it and fails after `limit`.
fn finishes(child: &mut Child, root: &Path, limit: Duration) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let log = std::fs::read_to_string(root.join("editor.err.log")).unwrap_or_default();
            panic!("the editor did not end by itself within {limit:?}:\n{log}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "starts the editor with a hidden window and needs a GPU: cargo test -p ara-editor --test bounded_runs -- --ignored"]
fn frames_ends_a_hidden_run_by_itself() {
    let root = common::project("bounded-frames");
    let mut child = spawn(&root, &["--frames", "20", "--size", "960x600"]);
    let status = finishes(&mut child, &root, Duration::from_secs(120));
    assert!(status.success(), "{status}");
}

#[test]
#[ignore = "opens an unfocused window and needs a GPU: cargo test -p ara-editor --test bounded_runs -- --ignored"]
fn a_screenshot_run_saves_the_window_with_its_scene_in_it() {
    let root = common::project("bounded-screenshot");
    let output = root.join("native");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&root)
        .args([
            "--frames",
            "4",
            "--screenshot",
            "--size",
            "1280x800",
            "--output",
        ])
        .arg(&output)
        .stdout(std::fs::File::create(root.join("editor.out.log")).unwrap())
        .stderr(std::fs::File::create(root.join("editor.err.log")).unwrap())
        .spawn()
        .unwrap();
    let status = finishes(&mut child, &root, Duration::from_secs(180));
    assert!(status.success(), "{status}");
    let picture = image::open(output.join("editor.png")).unwrap().to_rgba8();
    assert!(
        picture.width() >= 640 && picture.height() >= 400,
        "{:?}",
        picture.dimensions()
    );
    // The Scene is in it: the middle of the Scene view is the render, not the window's hole.
    let layout: Value =
        serde_json::from_slice(&std::fs::read(output.join("editor.layout.json")).unwrap()).unwrap();
    let view = layout["ui"]["elements"]
        .as_array()
        .or_else(|| layout["elements"].as_array())
        .unwrap()
        .iter()
        .find(|e| e["key"] == "scene/view")
        .expect("the layout names the Scene view");
    let rect: Vec<f64> = view["rect"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    let (x, y) = (
        (rect[0] + rect[2] / 2.0) as u32,
        (rect[1] + rect[3] / 2.0) as u32,
    );
    let [r, g, b, a] = picture.get_pixel(x, y).0;
    assert_eq!(a, 255, "opaque where the Scene is");
    assert!(
        u32::from(r) + u32::from(g) + u32::from(b) > 60,
        "the Scene's picture, not black: {r} {g} {b}"
    );
}

/// The saved layout has the Game tab in front of the Scene (`.ara/editor-layout.json`): the Scene
/// layer is never shown, so only the Game view can be presented. A bounded run still counts
/// frames and ends, and the picture is the window as laid out (without the Scene's image).
#[test]
#[ignore = "opens an unfocused window and needs a GPU: cargo test -p ara-editor --test bounded_runs -- --ignored"]
fn a_screenshot_run_ends_with_the_game_tab_in_front() {
    let root = common::project("bounded-game-tab");
    let area = |id: &str, tabs: Value, active: u32| json!({"Area": {"id": id, "tabs": tabs, "active": active}});
    let split = |axis: &str, ratio: f64, first: Value, second: Value| json!({"Split": {"axis": axis, "ratio": ratio, "first": first, "second": second}});
    let layout = json!({
        "version": 2,
        "layout": {
            "main": split(
                "Horizontal",
                0.16,
                area("outline", json!(["hierarchy"]), 0),
                split(
                    "Horizontal",
                    0.69,
                    split(
                        "Vertical",
                        0.72,
                        area("view", json!(["scene", "game"]), 1),
                        area("bottom", json!(["assets", "console"]), 0),
                    ),
                    area("right", json!(["inspector"]), 0),
                ),
            ),
            "floating": [],
        },
        "assets": {"sidebarWidth": 168, "expandedFolders": []},
    });
    std::fs::create_dir_all(root.join(".ara")).unwrap();
    std::fs::write(root.join(".ara/editor-layout.json"), layout.to_string()).unwrap();
    let output = root.join("native");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ara-editor"))
        .arg(&root)
        .args([
            "--frames",
            "4",
            "--screenshot",
            "--size",
            "1280x800",
            "--output",
        ])
        .arg(&output)
        .stdout(std::fs::File::create(root.join("editor.out.log")).unwrap())
        .stderr(std::fs::File::create(root.join("editor.err.log")).unwrap())
        .spawn()
        .unwrap();
    let status = finishes(&mut child, &root, Duration::from_secs(180));
    assert!(status.success(), "{status}");
    assert!(
        output.join("editor.png").is_file(),
        "the window picture is written"
    );
}

#[test]
#[ignore = "opens an unfocused window and needs a GPU: cargo test -p ara-editor --test bounded_runs -- --ignored"]
fn a_panel_screenshot_shows_the_page_in_the_main_window_and_leaves_the_layout_alone() {
    let root = common::project("bounded-panel");
    let layout = root.join(".ara/editor-layout.json");
    let _ = std::fs::remove_file(&layout);
    let output = root.join("native");
    let mut child = spawn(
        &root,
        &[
            "--panel",
            "vfx",
            "--screenshot",
            "--size",
            "1280x800",
            "--output",
            output.to_str().unwrap(),
        ],
    );
    // A page that took the Scene's place ends in seconds, not after the stall limit.
    let status = finishes(&mut child, &root, Duration::from_secs(40));
    assert!(status.success(), "{status}");
    let tree: Value =
        serde_json::from_str(&std::fs::read_to_string(output.join("editor.layout.json")).unwrap())
            .unwrap();
    assert!(
        tree["elements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["key"].as_str().is_some_and(|k| k.starts_with("vfx/"))),
        "the VFX page is in the main window"
    );
    assert!(
        !layout.exists(),
        "a bounded run never writes the project's window layout"
    );
}

#[test]
#[ignore = "starts the editor with a hidden window and needs a GPU: cargo test -p ara-editor --test bounded_runs -- --ignored"]
fn a_long_frame_run_is_a_protocol_server_with_no_window() {
    let root = common::project("bounded-server");
    let mut child = spawn(&root, &["--frames", "1000000", "--size", "640x480"]);
    let started = Instant::now();
    let mut client = loop {
        if let Some(client) = Client::for_project(&root) {
            break client;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "the editor ended early"
        );
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "no protocol server"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let session = client
        .call(
            "session.begin",
            json!({"client": "test", "author": "human", "label": "server"}),
        )
        .unwrap();
    let said = client
        .call("editor.imports", json!({"session": session["session"]}))
        .unwrap();
    assert!(said["stage"].is_string(), "{said}");
    assert!(child.try_wait().unwrap().is_none(), "still serving");
    let _ = child.kill();
    let _ = child.wait();
}
