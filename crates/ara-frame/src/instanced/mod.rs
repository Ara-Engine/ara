//! Model-space geometry shared by raster, fidelity and reference presentation.
//! Gameplay state remains in ECS; these are derived content and motion snapshots.
use glam::{Mat4, Vec3};

mod changes;
mod material;
mod textures;
mod validate;
pub use changes::{CasterChanges, Changes, FrameWork, transport};
pub use material::{Material, MaterialExtras, TextureSlot, TextureTransform, Volume};
pub use textures::{TextureBinding, TextureStats, Textures};
pub use validate::{CheckedScene, ValidatedScene};

/// Absent native texture or unused material index. Sampling supplies neutral
/// white/flat-normal texels without allocating default images in the heap.
pub const NO_TEXTURE: u32 = u32::MAX;

/// Native interleaved model-space vertex, exactly 64 bytes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Position in the geometry's local metre space.
    pub position: [f32; 3],
    /// Unit local normal, transformed with inverse transpose at sampling time.
    pub normal: [f32; 3],
    /// Original texture coordinate set zero.
    pub uv: [f32; 2],
    /// Authored/generated tangent xyz and bitangent handedness w.
    pub tangent: [f32; 4],
    /// Original texture coordinate set one.
    pub uv1: [f32; 2],
    /// Terrain's four splat bytes, little-endian; zero for model geometry.
    pub terrain_weights: u32,
    /// Linear RGBA8 vertex colour, red in the low byte ([`Vertex::WHITE`] without
    /// colours). Materials with `VERTEX_COLOR_TINT` multiply base colour by it and
    /// vertex-paint layer materials blend by its channels.
    pub color: u32,
}

impl Vertex {
    /// The colour of geometry without vertex colours: opaque white.
    pub const WHITE: u32 = u32::MAX;
}

/// Packs linear RGBA (clamped to 0..1) as a [`Vertex::color`] word.
pub fn pack_color(rgba: [f32; 4]) -> u32 {
    u32::from_le_bytes(rgba.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
}

/// One cooked triangle LOD, sharing the geometry's vertex allocation.
pub struct Lod {
    /// Triangle-list local vertex indices.
    pub indices: Vec<u32>,
    /// Maximum model-space geometric deviation in metres; zero for finest LOD.
    pub error: f32,
}

/// Unique geometry uploaded once, independently of its number of instances.
pub struct Geometry {
    /// Source identity covering rest positions, attributes and LOD topology.
    /// Include a selection pivot only if baked into these vertices; ordinary
    /// source selections share geometry and retain their pivot in Instance.world.
    /// Compute when content changes; never hash per frame.
    pub key: [u8; 32],
    /// Model-space source attributes; skinning updates only a deforming geometry.
    pub vertices: Vec<Vertex>,
    /// Finest first, with nondecreasing geometric error.
    pub lods: Vec<Lod>,
    /// Local axis-aligned minimum and maximum, including all source vertices.
    pub bounds: [Vec3; 2],
    /// Whether animation can replace positions/normals/tangents between frames.
    pub deforming: bool,
    /// Whether any vertex colour differs from [`Vertex::WHITE`]. False promises
    /// all-white vertices (validated), so tinting materials draw through the
    /// plain G-buffer PSO; true routes them to the vertex-colour PSO variant.
    pub vertex_colors: bool,
    /// Position/normal/tangent revision for deforming geometry. Advance after a
    /// new skin pose; static geometry retains zero and keeps its GPU allocation.
    pub revision: u64,
}

/// A stable draw instance, retaining its real world transform.
pub struct Instance {
    /// Stable external identity, distinct for each rendered primitive/foliage part.
    pub entity: u64,
    /// Capture legend label.
    pub name: String,
    /// Index of shared model-space geometry.
    pub geometry: usize,
    /// Index of the resolved PBR material.
    pub material: usize,
    /// Current local-to-world affine matrix, including foliage part transforms once.
    pub world: Mat4,
    /// Previous presented local-to-world matrix for motion reconstruction.
    pub previous_world: Mat4,
    /// Whether it blocks light. False still draws, reflects and bounces light, but
    /// leaves it out of every shadow map and ray-traced shadow.
    pub casts_shadows: bool,
    /// Per-instance vertex colours (Unreal's mesh-paint override colours): one
    /// [`Vertex::color`] word per geometry vertex, replacing the shared geometry's
    /// colours for this instance only. Every LOD indexes the same vertices, so the
    /// override covers all of them. `None` reads the geometry's colours.
    pub paint: Option<std::sync::Arc<[u32]>>,
    /// Whether decals project onto it (Unreal's bReceivesDecals).
    pub receives_decals: bool,
}

impl Instance {
    /// Transforms local culling bounds, including mirrored/nonuniform scale.
    pub fn bounds(&self, geometry: &Geometry) -> [Vec3; 2] {
        let centre = (geometry.bounds[0] + geometry.bounds[1]) * 0.5;
        let extent = (geometry.bounds[1] - geometry.bounds[0]) * 0.5;
        let centre = self.world.transform_point3(centre);
        let extent = self.world.x_axis.truncate().abs() * extent.x
            + self.world.y_axis.truncate().abs() * extent.y
            + self.world.z_axis.truncate().abs() * extent.z;
        [centre - extent, centre + extent]
    }
}

/// Engine-owned scene content consumed by both raster and fidelity views.
pub struct Scene {
    /// Unique model-space geometry, rather than one world-space mesh per draw.
    pub geometry: Vec<Geometry>,
    /// Stable instances. Native ray-query IDs are checked against the 24-bit ABI.
    pub instances: Vec<Instance>,
    /// Resolved materials, including independent coordinates for each map.
    pub materials: Vec<Material>,
    /// Number of registered stable texture IDs in the shared native resident set.
    pub textures: u32,
    /// Deferred decals; their material rows live in `materials`.
    pub decals: Vec<Decal>,
    /// Point and spot lights with the shared physical-unit contract.
    pub lights: Vec<crate::scene::Light>,
    /// Direction toward the sun.
    pub sun_direction: Vec3,
    /// Linear RGB sun illuminance in lux.
    pub sun_lux: Vec3,
    /// Linear RGBA32F equirectangular environment radiance in cd/m².
    pub environment: Vec<[f32; 4]>,
    /// Equirectangular environment width and height.
    pub environment_size: [u32; 2],
}

/// Opacity channels of a [`Decal`], in [`Decal::opacity`] order.
pub const DECAL_CHANNELS: [&str; 7] = [
    "base colour",
    "normal",
    "roughness",
    "metallic",
    "specular",
    "emissive",
    "occlusion",
];

/// One deferred decal (Unreal's DecalComponent): projects a material row along its
/// local -Z onto the opaque G-buffer inside its box, after the opaque pass and
/// before lighting. Surfaces of instances with `receives_decals` false, translucent
/// surfaces and the sky never receive.
#[derive(Debug, Clone)]
pub struct Decal {
    /// Stable external identity (capture legends, sort tie-break).
    pub entity: u64,
    /// Capture legend label.
    pub name: String,
    /// The box's unit cube (-0.5..0.5 on every axis) to world; affine, invertible.
    pub world: Mat4,
    /// Index of its material row in [`Scene::materials`]: base colour (slot 0),
    /// normal (1), roughness/metal (2, G/B), emissive (3), occlusion (4) and opacity
    /// (5) maps over the projected coordinates (each slot's transform applies),
    /// base colour/emissive/roughness/metal factors, and opacity factor in alpha.
    pub material: usize,
    /// Per-channel opacity (base colour, normal, roughness, metallic, specular,
    /// emissive, occlusion; [`DECAL_CHANNELS`]), each 0..1.
    pub opacity: [f32; 7],
    /// Channel (0-3) of the opacity map that scales opacity; 4 reads none.
    pub opacity_channel: u32,
    /// Ambient occlusion factor and the occlusion map channel it multiplies (4: none).
    pub occlusion: [f32; 2],
    /// Specular level (0.5 is 4% F0). The G-buffer has no specular channel yet.
    pub specular: f32,
    /// Camera distance (m) where the decal starts fading out and where it is gone;
    /// an end not past the start never fades.
    pub fade_distance: [f32; 2],
    /// Angle (degrees) between the surface normal and the projection axis where the
    /// decal starts fading, and where it is gone.
    pub fade_angle: [f32; 2],
    /// Overlapping decals draw in ascending order (later on top); entity breaks ties.
    pub sort_order: i32,
    /// Below this share of the view height the decal fades out and is skipped.
    pub fade_screen_size: f32,
}
