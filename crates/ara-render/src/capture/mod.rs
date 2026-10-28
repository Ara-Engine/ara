//! Deterministic headless requests, entity picking, marks and perceptual comparisons.
mod compare;
mod marks;
use crate::scene::CameraSpec;
use anyhow::{Result, ensure};
use ara_gpu::vulkan::{DeviceInfo, PassTiming};
pub use compare::{Comparison, compare};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Debug view IDs are also the cooked shader ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum ViewMode {
    /// Lit linear HDR, exposed and tone mapped to SDR.
    Lit = 0,
    /// Linear material base color, displayed in sRGB.
    Albedo = 1,
    /// World normals remapped from -1..1 to 0..1.
    Normals = 2,
    /// Reversed-Z depth, fourth-root display curve.
    Depth = 3,
    /// Red roughness, green metallic.
    RoughnessMetal = 4,
    /// Stable pseudocolors from render IDs.
    EntityIds = 5,
    /// Cascade selection multiplied by shadow visibility.
    ShadowCascades = 6,
    /// Triangle edges of the native geometry over a dark background, coloured per
    /// entity (no temporal jitter, so edges hold still).
    Wireframe = 7,
    /// The lit image with the native geometry's triangle edges drawn over it.
    LitWireframe = 8,
    /// The diffuse indirect light surface shading actually samples (probe GI, sky
    /// irradiance and the screen-space bounce, with ambient occlusion), without direct
    /// light, emission, specular reflection or fog: exposed like Lit, shown in sRGB
    /// without tone mapping. Opaque surfaces only (blended draws are not drawn);
    /// the background is black. Only where [`RenderCapabilities::gi_diffuse`] holds.
    GiDiffuse = 9,
    /// Raster fragments per pixel of the submitted native geometry before depth
    /// rejection (both faces: native draws cull none; masked materials' alpha discard
    /// respected), on the fixed heat legend: 0 black, 1 blue, 2 green, 4 yellow,
    /// 8 orange, 16 or more red. Captures carry the measured maximum and the pixels
    /// per legend band ([`OverdrawStats`]). Only where [`RenderCapabilities::overdraw`]
    /// holds.
    Overdraw = 10,
}
impl ViewMode {
    /// Complete baseline debug capture set.
    pub const ALL: [Self; 7] = [
        Self::Lit,
        Self::Albedo,
        Self::Normals,
        Self::Depth,
        Self::RoughnessMetal,
        Self::EntityIds,
        Self::ShadowCascades,
    ];
    /// Stable filename/agent mode identifier.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lit => "lit",
            Self::Albedo => "albedo",
            Self::Normals => "normals",
            Self::Depth => "depth",
            Self::RoughnessMetal => "roughness_metal",
            Self::EntityIds => "entity_ids",
            Self::ShadowCascades => "shadow_cascades",
            Self::Wireframe => "wireframe",
            Self::LitWireframe => "lit_wireframe",
            Self::GiDiffuse => "gi_diffuse",
            Self::Overdraw => "overdraw",
        }
    }
    /// Renders the lit scene (`Lit`, and under `LitWireframe`'s edges).
    pub fn lit(self) -> bool {
        matches!(self, Self::Lit | Self::LitWireframe)
    }
    /// Draws triangle edges over the image.
    pub fn wire(self) -> bool {
        matches!(self, Self::Wireframe | Self::LitWireframe)
    }
    /// The debug view ID shaders read (`Frame.temporal.z`): lit modes are 0.
    pub(crate) fn shader_mode(self) -> u32 {
        if self.lit() { 0 } else { self as u32 }
    }
}
/// What this renderer can show beyond Lit, with the reason for anything it cannot
/// (`Renderer::capabilities`; `render.probes.query` reports it as is). A mode it cannot
/// show is never drawn as Lit under that mode's name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderCapabilities {
    /// Authored reflection probes are captured and shade surfaces at the current
    /// preset (native raster at High or Epic).
    pub reflection_probes: bool,
    /// [`ViewMode::GiDiffuse`] is available and its GI data ready.
    pub gi_diffuse: bool,
    /// [`ViewMode::Overdraw`] is available.
    pub overdraw: bool,
    /// For each capability that is `false`, why (keys `reflectionProbes`,
    /// `giDiffuse`, `overdraw`).
    pub reasons: std::collections::BTreeMap<String, String>,
}
/// Agent-visible capture contract; no wall-clock values enter rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRequest {
    /// Arbitrary camera pose or bounds fitting instruction.
    pub camera: CameraSpec,
    /// Requested pixel width/height.
    pub size: [u32; 2],
    /// Lit or diagnostic channel.
    pub mode: ViewMode,
    /// Disable temporal jitter/history blending and lock time/exposure.
    pub deterministic: bool,
    /// Linear exposure multiplier, applied before the documented tone mapper.
    pub exposure: f32,
    /// Frames after history reset; at least one is always rendered.
    pub settle_frames: u32,
    /// Fixed simulation tick supplied by the host.
    pub tick: u64,
    /// Replay/scene random seed supplied by the host.
    pub seed: u64,
    /// Maximum labelled entities; None disables marks.
    pub marks: Option<usize>,
}
/// One numbered, visible entity in a labelled capture.
#[derive(Debug, Serialize)]
pub struct LegendEntry {
    /// Number rendered on the image.
    pub mark: usize,
    /// Stable external ECS/document identity.
    pub entity: u64,
    /// Human-readable scene name.
    pub name: String,
    /// Visible-pixel bounding box [x, y, width, height].
    pub bbox: [u32; 4],
}
/// Reproduction data written next to every capture.
#[derive(Serialize)]
pub struct CaptureMetadata {
    /// Renderer or feature content identity, including contributing scene assets.
    pub scene_hash: String,
    /// Complete deterministic request.
    pub request: CaptureRequest,
    /// GPU channel index to stable external identity and name.
    pub entity_map: Vec<(u32, u64, String)>,
    /// Offline shader/compiler/layout provenance for this complete baseline cook.
    pub shaders: ShaderMetadata,
    /// Native device/driver/cache/timestamp provenance.
    pub device: serde_json::Value,
    /// Actual queue timestamps for the final settle frame.
    pub timings: Vec<PassTiming>,
    /// The overdraw view's measured counts (`ViewMode::Overdraw` captures only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overdraw: Option<OverdrawStats>,
}
/// Fragments per render pixel of an overdraw capture (`ViewMode::Overdraw`), counted
/// before depth rejection with alpha discard respected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverdrawStats {
    /// The most fragments any pixel received.
    pub max: u32,
    /// Render pixels counted.
    pub pixels: u64,
    /// Pixels per legend band: 0, 1, 2-3, 4-7, 8-15, 16 or more fragments.
    pub bands: [u64; 6],
}
impl OverdrawStats {
    /// Statistics of per-pixel fragment `counts`.
    pub fn of(counts: impl IntoIterator<Item = u32>) -> Self {
        let mut stats = Self {
            max: 0,
            pixels: 0,
            bands: [0; 6],
        };
        for count in counts {
            stats.max = stats.max.max(count);
            stats.pixels += 1;
            // 0 -> band 0, 1 -> 1, 2-3 -> 2, 4-7 -> 3, 8-15 -> 4, 16+ -> 5.
            stats.bands[(u32::BITS - count.leading_zeros()).min(5) as usize] += 1;
        }
        stats
    }
}
/// Immutable offline shader provenance; no runtime compiler participates.
#[derive(Clone, Serialize)]
pub struct ShaderMetadata {
    /// Source, compiler, manifest and binding ABI content key.
    pub content_key: String,
    /// Slang version recorded by the cook.
    pub compiler: String,
    /// Hash of the shared native-backend binding contract.
    pub binding_layout_hash: String,
}
pub(crate) struct CaptureContext<'a> {
    pub names: &'a [(u64, String)],
    pub hash: &'a str,
    pub device: &'a DeviceInfo,
    pub timings: &'a [PassTiming],
    pub shaders: &'a ShaderMetadata,
}
/// Owned capture channels with stable pixel-to-entity lookup.
pub struct Capture {
    /// Unlabelled display image.
    pub image: RgbaImage,
    /// Numbered display image, if marks were requested.
    pub marked: Option<RgbaImage>,
    /// Mark to entity mapping with visible bounds.
    pub legend: Vec<LegendEntry>,
    /// Reproduction metadata.
    pub metadata: CaptureMetadata,
    entities: Vec<Option<u64>>,
    render_ids: Vec<u32>,
}
/// Channels and provenance from an external render-graph feature, such as VFX.
pub struct FeatureCapture<'a> {
    /// Tightly packed RGBA8 display pixels.
    pub rgba: Vec<u8>,
    /// Compact feature IDs; zero is background, one-based indices address names.
    pub ids: Vec<u32>,
    /// Stable external identities and labels.
    pub names: &'a [(u64, String)],
    /// Deterministic source content hash.
    pub hash: &'a str,
    /// Actual native device provenance.
    pub device: &'a DeviceInfo,
    /// Measured feature pass timestamps.
    pub timings: &'a [PassTiming],
    /// Matching offline shader provenance.
    pub shaders: &'a ShaderMetadata,
}
impl Capture {
    /// Assemble feature-owned GPU readbacks using the standard capture contract.
    pub fn from_feature(input: FeatureCapture<'_>, request: &CaptureRequest) -> Result<Self> {
        Self::from_render(
            input.rgba,
            input.ids,
            request,
            CaptureContext {
                names: input.names,
                hash: input.hash,
                device: input.device,
                timings: input.timings,
                shaders: input.shaders,
            },
        )
    }
    pub(crate) fn from_render(
        rgba: Vec<u8>,
        ids: Vec<u32>,
        request: &CaptureRequest,
        context: CaptureContext<'_>,
    ) -> Result<Self> {
        let CaptureContext {
            names,
            hash,
            device,
            timings,
            shaders,
        } = context;
        ensure!(
            ids.len() == request.size[0] as usize * request.size[1] as usize,
            "Invalid entity readback"
        );
        ensure!(
            ids.iter().all(|i| *i == 0 || (*i as usize) <= names.len()),
            "Invalid GPU entity ID"
        );
        let image = RgbaImage::from_raw(request.size[0], request.size[1], rgba)
            .ok_or_else(|| anyhow::anyhow!("Invalid RGBA readback"))?;
        let entities = ids
            .iter()
            .map(|id| {
                if *id == 0 {
                    None
                } else {
                    Some(names[*id as usize - 1].0)
                }
            })
            .collect();
        let (marked, legend) = if let Some(max) = request.marks {
            let (image, legend) = marks::label(&image, &ids, names, max);
            (Some(image), legend)
        } else {
            (None, Vec::new())
        };
        Ok(Self {
            image,
            marked,
            legend,
            entities,
            render_ids: ids,
            metadata: CaptureMetadata {
                scene_hash: hash.into(),
                request: request.clone(),
                shaders: shaders.clone(),
                entity_map: names
                    .iter()
                    .enumerate()
                    .map(|(i, (id, name))| (i as u32 + 1, *id, name.clone()))
                    .collect(),
                device: ara_json::to_value(device)?,
                timings: timings.to_vec(),
                overdraw: None,
            },
        })
    }
    /// Pick the stable entity at a pixel. Background and out-of-bounds return None.
    pub fn entity_at(&self, x: u32, y: u32) -> Option<u64> {
        if x >= self.image.width() || y >= self.image.height() {
            return None;
        }
        self.entities[y as usize * self.image.width() as usize + x as usize]
    }
    /// Save PNG, optional marks PNG, legend JSON, metadata and little-endian entity IDs.
    /// Raw U32 render IDs use zero for background; metadata contains stable identity mappings.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.image.save(path)?;
        if let Some(marked) = &self.marked {
            marked.save(path.with_extension("marks.png"))?;
        }
        ara_gpu::cooked::write_json(&path.with_extension("legend.json"), &self.legend)?;
        ara_gpu::cooked::write_json(&path.with_extension("json"), &self.metadata)?;
        let bytes = self
            .render_ids
            .iter()
            .flat_map(|id| id.to_le_bytes())
            .collect::<Vec<_>>();
        std::fs::write(path.with_extension("render_ids.bin"), bytes)?;
        Ok(())
    }
}
