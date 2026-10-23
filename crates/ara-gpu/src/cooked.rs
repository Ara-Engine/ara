//! Versioned offline pipeline descriptions, consumed without a shader compiler.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Binding ABI version one. Changes invalidate every cooked pipeline.
pub const BINDING_LAYOUT: &str = "ara.v1:root32:texture128:buffer64:sampler3:storage64:as16";
/// Hash of the exact global binding ABI.
pub fn binding_layout_hash() -> String {
    format!("{:x}", Sha256::digest(BINDING_LAYOUT.as_bytes()))
}

/// A shader stage produced by the offline cook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookedStage {
    /// Source entry point, also the basename for artifacts.
    pub source_entry: String,
    /// Slang emits `main` in SPIR-V.
    pub spirv_entry: String,
    /// Vertex, fragment or compute.
    pub stage: Stage,
    /// Relative SPIR-V artifact path.
    pub spirv: String,
    /// Relative DXIL artifact path (Metal converter input).
    pub dxil: String,
    /// SHA-256 of the SPIR-V bytes.
    pub spirv_hash: String,
    /// SHA-256 of the DXIL bytes.
    pub dxil_hash: String,
    /// Optional offline Metal Shader Converter artifacts, filled by the Mac cook lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metal: Option<CookedMetalStage>,
}
/// Converter-specific native artifact and binding provenance for Metal 4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookedMetalStage {
    /// Relative metallib artifact path; never compile DXIL at runtime.
    pub metallib: String,
    /// SHA-256 of the metallib bytes.
    pub metallib_hash: String,
    /// Relative converter reflection JSON, separate from Slang reflection.
    pub reflection: String,
    /// Exact converter version and content key.
    pub converter: String,
    /// Hash of converter root signature and stage-in ABI.
    pub root_signature_hash: String,
}
/// Pipeline shader stage.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Raster vertex transform.
    Vertex,
    /// Raster surface evaluation.
    Fragment,
    /// Compute dispatch.
    Compute,
}
impl Stage {
    /// Slang's command line stage name.
    pub fn slang_name(self) -> &'static str {
        match self {
            Self::Vertex => "vertex",
            Self::Fragment => "fragment",
            Self::Compute => "compute",
        }
    }
}
/// Portable raster state with formats encoded as Vulkan numeric format IDs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RenderState {
    /// Color attachment formats, in shader output order.
    pub color_formats: Vec<i32>,
    /// Optional depth format.
    pub depth_format: Option<i32>,
    /// Vertex stride; zero denotes a fullscreen triangle.
    pub vertex_stride: u32,
    /// Reversed-Z depth testing.
    pub depth_test: bool,
    /// Depth writes disabled for transparent passes.
    pub depth_write: bool,
    /// Premultiplied alpha blending.
    pub blend: bool,
}
/// Runtime pipeline manifest, content-addressed by the cook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookedPipeline {
    /// Manifest schema version.
    pub version: u32,
    /// Compiler, source, entry points, layouts and state content hash.
    pub key: String,
    /// Hash of the shared root/descriptor ABI.
    pub binding_layout_hash: String,
    /// Exact compiler version used for the artifacts.
    pub compiler: String,
    /// Per-stage artifacts and hashes.
    pub stages: Vec<CookedStage>,
    /// Raster formats, vertex ABI and render state.
    pub state: RenderState,
}

/// Write derived JSON with recursively sorted keys and one trailing newline.
/// Engine documents use ara-doc; this writer is only for renderer/cook artifacts.
pub fn write_json(path: &std::path::Path, value: &impl Serialize) -> anyhow::Result<()> {
    fn sorted(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => serde_json::Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, sorted(v)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            serde_json::Value::Array(array) => {
                serde_json::Value::Array(array.into_iter().map(sorted).collect())
            }
            value => value,
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&sorted(serde_json::to_value(value)?))?;
    bytes.push(b'\n');
    std::fs::write(path, bytes)?;
    Ok(())
}
