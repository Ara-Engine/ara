//! Offline-only material generation and pipeline cooking.
use anyhow::Result;
use ara_gpu::cooked::{RenderState, Stage};
use ara_shader_cook::{PipelineSource, Toolchain};
use std::{collections::BTreeMap, path::Path};
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        root.join("shaders/virtual_texture.slang"),
        ara_texture_stream::shader_source(),
    )?;
    std::fs::write(
        root.join("shaders/terrain_material.slang"),
        ara_landscape::material::evaluator()?,
    )?;
    let mut manifest = BTreeMap::new();
    for (name, entry) in [
        ("tile_upload", "tileUpload"),
        ("reset", "resetMain"),
        ("select", "selectMain"),
        ("terrain_cull", "terrainCull"),
        ("foliage_cull", "foliageCull"),
        ("hzb", "hzbMain"),
    ] {
        manifest.insert(
            name.to_owned(),
            PipelineSource {
                source: "landscape.slang".into(),
                entries: BTreeMap::from([(entry.into(), Stage::Compute)]),
                state: RenderState::default(),
                defines: BTreeMap::new(),
            },
        );
    }
    for (name, vertex, fragment, color, depth_write) in [
        (
            "terrain_depth",
            "terrainDepthVertex",
            "terrainDepth",
            false,
            true,
        ),
        (
            "terrain_shadow",
            "terrainDepthVertex",
            "terrainDepth",
            false,
            true,
        ),
        (
            "terrain_color",
            "terrainVertex",
            "terrainColor",
            true,
            false,
        ),
        (
            "foliage_depth",
            "foliageVertex",
            "foliageDepth",
            false,
            true,
        ),
        (
            "foliage_shadow",
            "foliageVertex",
            "foliageDepth",
            false,
            true,
        ),
        (
            "foliage_color",
            "foliageVertex",
            "foliageColor",
            true,
            false,
        ),
        ("sky", "skyVertex", "skyColor", true, false),
    ] {
        manifest.insert(
            name.to_owned(),
            PipelineSource {
                source: "landscape.slang".into(),
                entries: BTreeMap::from([
                    (vertex.into(), Stage::Vertex),
                    (fragment.into(), Stage::Fragment),
                ]),
                state: RenderState {
                    color_formats: if color { vec![37] } else { vec![] },
                    depth_format: if name == "sky" { None } else { Some(126) },
                    vertex_stride: 0,
                    depth_test: name != "sky",
                    depth_write,
                    blend: false,
                },
                defines: BTreeMap::new(),
            },
        );
    }
    // PipelineSource is a deserialization-only external type; use the canonical
    // cooked-artifact writer for the matching manifest wire representation.
    let value: serde_json::Value = manifest
        .iter()
        .map(|(name, p)| {
            (
                name.to_owned(),
                serde_json::json!({"source":p.source,"entries":p.entries,"state":p.state}),
            )
        })
        .collect();
    let path = root.join("shaders/pipelines.json");
    ara_gpu::cooked::write_json(&path, &value)?;
    let sdk = std::env::var_os("VULKAN_SDK")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:/VulkanSDK/1.4.363.0".into());
    ara_shader_cook::cook(
        &path,
        &root.join("cooked"),
        &Toolchain {
            slangc: sdk.join("Bin/slangc.exe"),
            spirv_val: sdk.join("Bin/spirv-val.exe"),
        },
    )?;
    Ok(())
}
