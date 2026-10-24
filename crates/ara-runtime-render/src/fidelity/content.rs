//! Resolved assets at the presentation upload boundary.
use ara_render::scene::{Material, Texture};
use ara_render_fidelity::material::MaterialDetail;
use ara_runtime::model::Model;
use std::{collections::BTreeMap, sync::Arc};

/// Assets needed in addition to the runtime frame. Callers resolve these once per
/// content generation; the adapter does not cook assets or read project files.
#[derive(Default)]
pub struct Content {
    /// Resolved terrain surfaces. The transitional converter takes the first two
    /// as ground and rock; authored four-channel splats replace this in the scale pass.
    pub terrain: Vec<TerrainLayer>,
    /// Foliage rules paired with their actual cooked model assets.
    pub foliage: Vec<Foliage>,
    /// Authored exclusion zones used during deterministic scattering.
    pub exclusions: Vec<ara_foliage::Exclusion>,
    /// Optional material layers indexed by model asset identity.
    pub details: BTreeMap<String, MaterialDetail>,
    /// HDR sky override; absent uses the frame's Environment gradient.
    pub environment: Option<EnvironmentMap>,
}

/// A resolved terrain surface material, including all of its PBR maps.
pub struct TerrainLayer {
    /// Linear PBR factors and alpha mode.
    pub material: Material,
    /// Base colour, normal, metallic/roughness and emissive images.
    pub textures: [Option<Texture>; 4],
    /// World metres covered by one UV repetition along X and Z.
    pub meters: [f32; 2],
}

/// One foliage rule's source model, shared by all scattered instances.
pub struct Foliage {
    /// Authored placement rule, retaining its asset and deterministic seed.
    pub rule: ara_foliage::Rule,
    /// Decoded cooked generation of the rule's model.
    pub model: Arc<Model>,
}

/// Equirectangular sky radiance in cd/m².
#[derive(Clone)]
pub struct EnvironmentMap {
    /// Width and height in texels.
    pub size: [u32; 2],
    /// Linear RGBA32F pixels.
    pub pixels: Vec<[f32; 4]>,
}
