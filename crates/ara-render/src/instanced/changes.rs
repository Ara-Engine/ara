//! Publication-local indices for incremental native frame updates.

/// Render-v3 event source and portable lab replay API. The legacy row lists stay independent.
pub mod transport {
    pub use crate::look::transport::changes::*;
}

/// Rows changed since the preceding native update. Geometry, material and
/// instance layout must still match the published scene; structural edits replace
/// the scene. Producers retain these vectors and clear them between frames.
#[derive(Default)]
pub struct Changes {
    /// Instances whose world matrix changed.
    pub instances: Vec<usize>,
    /// Resolved material blocks whose factors or sampling policy changed.
    pub materials: Vec<usize>,
    /// Deforming geometries whose vertex revision changed.
    pub geometries: Vec<usize>,
    /// Instances whose per-instance paint colours changed (`Instance::paint`, same
    /// length). Adding or removing paint changes grouping: replace the scene.
    pub paint: Vec<usize>,
    /// Whether any decal's transform or parameters changed (same count and order).
    pub decals: bool,
}

impl Changes {
    /// Reuse storage for the next extraction.
    pub fn clear(&mut self) {
        self.instances.clear();
        self.materials.clear();
        self.geometries.clear();
        self.paint.clear();
        self.decals = false;
    }
}

/// Caster invalidation from the most recently prepared frame. Bounds cover both
/// old and new positions, including skin, material/alpha and selected-LOD changes.
pub struct CasterChanges<'a> {
    /// A new content generation invalidates every cached shadow.
    pub all: bool,
    /// World-space old-union-new bounds of changed instances.
    pub bounds: &'a [[glam::Vec3; 2]],
}

/// Work actually performed during the last native frame, for performance probes.
#[derive(Clone, Copy, Default, Debug)]
pub struct FrameWork {
    /// Instance rows written, including one-frame motion history settlement.
    pub instance_rows: usize,
    /// Material blocks written.
    pub material_rows: usize,
    /// Instances tested against a pass frustum.
    pub cull_tests: usize,
    /// Instances whose camera-selected LOD was recomputed.
    pub lod_tests: usize,
    /// Visible instance/geometry pairs written.
    pub visible_rows: usize,
}
