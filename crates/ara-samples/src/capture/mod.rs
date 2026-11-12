//! `capture`: one still of a sample scene through the player's Renderlab renderer.
//! The in-process fidelity harness and the path-traced reference are gone.

use crate::play::Mcp;
use anyhow::{Context, Result};
use serde_json::json;
use std::path::Path;

/// What `capture` should do.
pub struct Request<'a> {
    /// Project folder under `samples/`.
    pub project: &'a str,
    /// Look name (`look/<name>.look.json`); its `scene` is what the player opens.
    pub look: &'a str,
}

/// Captures one look's scene with the pooled player's default renderer (Renderlab).
pub fn run(samples: &Path, request: &Request<'_>) -> Result<()> {
    let root = samples.join(request.project);
    let path = root
        .join(ara_doc::AUTHORING_ROOT)
        .join("look")
        .join(format!("{}.look.json", request.look));
    let look: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("parsing {}", path.display()))?;
    let scene = look["scene"]
        .as_str()
        .with_context(|| format!("{} has no scene", path.display()))?;
    let ticks = look["ticks"].as_u64().unwrap_or(60);
    std::fs::create_dir_all(root.join(".ara/samples"))?;
    let evidence = crate::evidence_directory(request.project);
    std::fs::create_dir_all(&evidence)?;
    let mut mcp = Mcp::start(&root)?;
    let start = mcp.play("start", json!({"scene": scene, "window": false, "seed": 7}))?;
    let world = start["world"].clone();
    if ticks > 0 {
        mcp.play("step", json!({"world": world, "ticks": ticks}))?;
    }
    let out = evidence.join(format!("{}-renderlab.png", request.look));
    mcp.play(
        "capture",
        json!({"world": world, "path": out.to_string_lossy(), "width": 1280, "height": 720, "marks": 0}),
    )?;
    mcp.play("stop", json!({"world": world}))?;
    eprintln!(
        "captured {scene} with the player's Renderlab renderer to {}",
        out.display()
    );
    Ok(())
}
