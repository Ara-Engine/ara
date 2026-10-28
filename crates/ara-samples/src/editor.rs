//! `editor`: the project open in `ara-editor`, captured for the editor team. The
//! editor runs in screenshot mode (a fixed frame count, then `editor.png`); while it
//! runs, an agent session on its Engine Protocol frames the Scene view
//! (`editor.camera`) and reads the console (`logs`), as any agent would.
use crate::build::tool;
use anyhow::{Context, Result, bail};
use ara_proto::Endpoint;
use ara_rpc::Client;
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

/// What to open and how to frame it.
pub struct Request<'a> {
    /// Project folder under `samples/`.
    pub project: &'a str,
    /// Scene the editor opens (the project's main scene is switched for the run).
    pub scene: &'a str,
    /// Capture file stem.
    pub name: &'a str,
    /// Orbit target, yaw and pitch (radians) and distance of the Scene view.
    pub camera: ([f32; 3], f32, f32, f32),
}

/// Runs the editor on the project and writes `evidence/<name>-editor.png` plus the
/// console summary it reported.
pub fn run(samples: &Path, request: &Request<'_>) -> Result<Value> {
    let root = samples.join(request.project);
    let settings = root.join("ara.project.json");
    let original = std::fs::read_to_string(&settings)?;
    let mut project: Value = serde_json::from_str(&original)?;
    project["main"] = json!(request.scene);
    std::fs::write(&settings, serde_json::to_string_pretty(&project)? + "\n")?;
    let result = capture(&root, request);
    std::fs::write(&settings, original)?;
    result
}

fn capture(root: &Path, request: &Request<'_>) -> Result<Value> {
    let output = root.join(".ara/samples/editor");
    let _ = std::fs::remove_dir_all(&output);
    // Open it as a fresh clone does: no local journal, so the build's agent edits are
    // plain content rather than an unreviewed overlay (agents cannot accept them).
    let _ = std::fs::remove_file(root.join(".ara/journal"));
    let mut child = Command::new(tool("ara-editor", "ARA_EDITOR")?)
        .arg(root)
        .args([
            "--screenshot",
            "--frames",
            "2400",
            "--size",
            "2560x1440",
            "--output",
        ])
        .arg(&output)
        .stdout(std::fs::File::create(root.join(".ara/samples/editor.log"))?)
        .stderr(std::fs::File::create(
            root.join(".ara/samples/editor.err.log"),
        )?)
        .spawn()
        .context("starting ara-editor")?;
    // The editor publishes `.ara/daemon.json` once its authority listens.
    let started = Instant::now();
    let mut client = loop {
        if let Some(client) = Client::for_project(root) {
            break client;
        }
        if let Some(status) = child.try_wait()? {
            bail!("ara-editor exited early ({status}); see .ara/samples/editor.err.log");
        }
        if started.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            bail!("the editor did not start its Engine Protocol server");
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let call = |client: &mut Client, method: &str, params: Value| -> Result<Value> {
        client
            .call(method, params)
            .map_err(|e| anyhow::anyhow!("{method}: {e:?}"))
    };
    // Agent sessions move their own presence camera, never the human's viewport
    // (D21); this navigation-only session frames the screenshot as the human would.
    let session = call(
        &mut client,
        "session.begin",
        json!({"client": "ara-samples", "author": "human", "label": "screenshot framing"}),
    )?;
    let handle = session["session"].clone();
    let ([x, y, z], yaw, pitch, distance) = request.camera;
    call(
        &mut client,
        "editor.camera",
        json!({"session": handle, "target": [x, y, z], "yaw": yaw, "pitch": pitch, "distance": distance}),
    )?;
    // Let imports, thumbnails and the fidelity view settle before reading the console.
    std::thread::sleep(Duration::from_secs(8));
    let logs =
        call(&mut client, "logs", json!({"session": handle, "limit": 40})).unwrap_or(Value::Null);
    drop(client);
    let status = child.wait()?;
    if !status.success() {
        bail!("ara-editor failed ({status}); see .ara/samples/editor.err.log");
    }
    let image = image::open(output.join("editor.png")).context("editor.png")?;
    let evidence = crate::evidence_directory(request.project);
    std::fs::create_dir_all(&evidence)?;
    image
        .resize(1920, 1080, image::imageops::FilterType::Lanczos3)
        .save(evidence.join(format!("{}-editor.png", request.name)))?;
    Ok(json!({"scene": request.scene, "camera": request.camera, "logs": logs}))
}
