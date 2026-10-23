#![warn(missing_docs)]
//! Offline, border-preserving cluster DAGs and independently decodable pages.
mod cook;
mod diagnostics;
mod format;
mod importer;
pub use diagnostics::Diagnostic;
pub mod sample;

use bytemuck::{Pod, Zeroable};
pub use cook::cook;
pub use format::{Geometry, PackedVertex, Page};
pub use importer::GeoImporter;
use serde::{Deserialize, Serialize};

/// Cooked format and algorithm revision, included in asset action identities.
pub const VERSION: u32 = 1;
/// Cluster-local vertex capacity.
pub const MAX_VERTICES: usize = 64;
/// Cluster-local triangle capacity.
pub const MAX_TRIANGLES: usize = 128;
/// Sentinel for a cluster without a coarser replacement.
pub const ROOT: u32 = u32::MAX;

/// Decoded static source vertex; positions use mesh-local metres.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, Serialize, Deserialize)]
pub struct Vertex {
    /// Mesh-local position.
    pub position: [f32; 3],
    /// Shading normal, normalized during packing.
    pub normal: [f32; 3],
    /// First texture coordinate set.
    pub uv: [f32; 2],
}
/// A single opaque material domain; split material/attribute seams before cooking.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMesh {
    /// Source vertices; identical seams should share vertex indices.
    pub vertices: Vec<Vertex>,
    /// Triangle-list indices.
    pub indices: Vec<u32>,
}
/// Offline cook policy; every value participates in the cache identity.
#[derive(Clone, Copy, Serialize, Deserialize, ara_schema::Schema)]
#[ara(name = "VirtualGeometrySettings", module = "geo")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Settings {
    /// Desired neighboring clusters per simplification group (2..=32).
    #[ara(default = 8, range(min = 2, max = 32))]
    pub group_size: u32,
    /// Maximum hierarchy depth (1..=32).
    #[ara(default = 24, range(min = 1, max = 32))]
    pub max_levels: u32,
    /// Relative error allowed in a single reduction (0..=1).
    #[ara(default = 0.1, range(min = 0, max = 1))]
    pub relative_error: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            group_size: 8,
            max_levels: 24,
            relative_error: 0.1,
        }
    }
}
/// Enclosing sphere shared by every cluster participating in one DAG decision.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Sphere {
    /// Mesh-local centre.
    pub center: [f32; 3],
    /// Conservative radius in metres.
    pub radius: f32,
}
/// A replacement group. Children are replaced together at this error threshold.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Group {
    /// Bounds enclosing source geometry and descendant group bounds.
    pub bounds: Sphere,
    /// Cumulative object-space error, including position quantization.
    pub error: f32,
    /// First member of the shared child-index array.
    pub first_child: u32,
    /// Number of finer clusters replaced by this group.
    pub child_count: u32,
    /// Replacement level; leaf geometry is level zero.
    pub level: u32,
}
/// One independently streamed cluster and its DAG links.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Cluster {
    /// Conservative geometry bounds.
    pub bounds: Sphere,
    /// Group which generated this cluster, or ROOT for original triangles.
    pub group: u32,
    /// Group replacing this cluster, or ROOT for permanently resident roots.
    pub parent: u32,
    /// Number of local vertices (at most 64).
    pub vertex_count: u32,
    /// Number of local triangles (at most 128).
    pub triangle_count: u32,
    /// Byte offset in the compressed page payload.
    pub offset: u32,
    /// Compressed vertex byte count.
    pub vertex_bytes: u32,
    /// Compressed index byte count.
    pub index_bytes: u32,
    /// Hierarchy level, useful for capture visualization.
    pub level: u32,
}
/// Diagnostics produced by an actual cook, without machine-specific timing.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct Diagnostics {
    /// Original triangle count, excluding no geometry.
    pub source_triangles: u64,
    /// Number of original meshlets.
    pub leaf_clusters: u32,
    /// Groups unable to reduce, usually because of borders or disconnected input.
    pub stalled_groups: u32,
    /// Deepest generated level.
    pub levels: u32,
    /// Largest position quantization displacement, in local metres.
    pub quantization_error: f32,
}
