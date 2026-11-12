//! Reuse metadata a replacement scene's geometry pool borrows from the pool it replaces.
use super::*;

/// Small immutable reuse metadata, captured at a content boundary. No vertices
/// or indices are copied; the previous pool remains owned by its live view.
pub(crate) struct GeometrySeed {
    pub(super) static_pool: Option<Buffer>,
    pub(super) layout: Vec<GeometryLayout>,
    pub(super) ranges: Vec<LodRange>,
    pub(super) lod_data: Vec<pack::PackedLod>,
}

impl GeometryGpu {
    pub(crate) fn preparation_seed(&self) -> GeometrySeed {
        GeometrySeed {
            static_pool: self.static_pool,
            layout: self.layout.clone(),
            ranges: self.ranges.clone(),
            lod_data: self.lod_data.clone(),
        }
    }
}
