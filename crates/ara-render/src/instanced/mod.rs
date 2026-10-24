//! Model-space geometry shared by raster, fidelity and reference presentation.
//! Gameplay state remains in ECS; these are derived content and motion snapshots.
use glam::{Mat4, Vec3};

mod changes;
mod geometry_gpu;
mod material;
#[cfg(test)]
mod tests;
mod textures;
mod validate;
pub use changes::{CasterChanges, Changes, FrameWork};
pub use geometry_gpu::{GeometryGpu, GeometryUpload, LodRange, PreparedPose};
pub use material::{Material, MaterialExtras, TextureSlot, TextureTransform};
pub use textures::{TextureBinding, TextureStats, Textures};

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
    /// Reserved shader word; must be zero.
    pub reserved: u32,
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
