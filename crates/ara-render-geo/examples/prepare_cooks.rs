//! Synchronize shared sampling source and canonicalize offline pipeline manifests.
use anyhow::Result;
use ara_gpu::cooked::write_json;
use serde_json::{Value, json};
use std::path::PathBuf;
fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        root.join("shaders/virtual_texture.slang"),
        ara_texture_stream::shader_source(),
    )?;
    let path = root.join("shaders/pipelines.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    manifest["display"] = json!({"entries":{"displayMain":"compute"},"source":"display.slang","state":{"blend":false,"color_formats":[],"depth_format":null,"depth_test":false,"depth_write":false,"vertex_stride":0}});
    manifest["resolve_textured"] = manifest["resolve"].clone();
    manifest["resolve_textured"]["entries"] = json!({"resolveTexturedMain":"compute"});
    write_json(&path, &manifest)?;
    let path = root.join("../ara-texture-stream/shaders/pipelines.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    write_json(&path, &manifest)?;
    Ok(())
}
