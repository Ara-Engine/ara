//! Deterministically ordered, narrowphase-only scene queries.
use crate::{BodyHandle, PhysicsPose, ShapeHandle};

/// Query inclusion policy. Filtering is applied before hit selection.
#[derive(Debug, Clone, Copy)]
pub struct QueryFilter {
    /// Inclusion bits for the sixteen collision layers.
    pub layer_mask: u32,
    /// Include triggers when true.
    pub sensors: bool,
    /// Body to exclude, e.g. the querying actor's own body.
    pub ignore: Option<BodyHandle>,
}
impl Default for QueryFilter {
    fn default() -> Self {
        Self {
            layer_mask: 0xffff,
            sensors: true,
            ignore: None,
        }
    }
}

/// One query in a batch. Displacements include the full requested distance.
#[derive(Debug, Clone, Copy)]
pub enum Query {
    /// All ray intersections, sorted by fraction/body/subshape.
    Ray {
        /// World-space ray origin.
        origin: [f32; 3],
        /// Full ray displacement.
        displacement: [f32; 3],
        /// Inclusion policy.
        filter: QueryFilter,
    },
    /// Sweep convex geometry (or a compound of convex shapes) against the world.
    Cast {
        /// Cooked query geometry.
        shape: ShapeHandle,
        /// Starting shape origin and rotation.
        pose: PhysicsPose,
        /// Full displacement.
        displacement: [f32; 3],
        /// Inclusion policy.
        filter: QueryFilter,
    },
    /// Overlap convex query geometry against all collider kinds, sorted by body/subshape.
    Overlap {
        /// Cooked query geometry.
        shape: ShapeHandle,
        /// Shape origin and rotation.
        pose: PhysicsPose,
        /// Inclusion policy.
        filter: QueryFilter,
    },
}

/// A surface hit; the normal points out of the hit body towards the query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueryHit {
    /// Runtime body identity.
    pub body: BodyHandle,
    /// Stable application identity, supplied when the body was created.
    pub user_data: u64,
    /// Leaf shape identity.
    pub subshape: u32,
    /// Ray/cast fraction in 0..1; zero for overlaps.
    pub fraction: f32,
    /// Penetration in meters for overlaps or casts starting inside.
    pub penetration: f32,
    /// World-space contact point.
    pub point: [f32; 3],
    /// Outward surface normal.
    pub normal: [f32; 3],
}

/// Packed batch results. Each range is available through `get`; no per-query allocation.
#[derive(Debug, Default)]
pub struct QueryResults {
    /// All hits in query order, each query's hits deterministically sorted.
    pub hits: Vec<QueryHit>,
    /// Start offsets, including the final sentinel.
    pub offsets: Vec<usize>,
}
impl QueryResults {
    /// Hits for the query at this index, or an empty slice if absent.
    pub fn get(&self, index: usize) -> &[QueryHit] {
        self.offsets
            .get(index..index.saturating_add(2))
            .and_then(|r| self.hits.get(r[0]..r[1]))
            .unwrap_or(&[])
    }
}
