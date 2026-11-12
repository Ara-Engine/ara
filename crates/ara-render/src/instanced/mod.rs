//! Model-space geometry shared by raster, fidelity and reference presentation.
//! The CPU records live in `ara-frame` and are re-exported here.
pub use ara_frame::instanced::{
    CasterChanges, Changes, Decal, FrameWork, Geometry, Instance, Lod, Material, MaterialExtras,
    Scene, TextureBinding, TextureSlot, TextureStats, TextureTransform, Textures, Vertex, Volume,
    pack_color, transport, DECAL_CHANNELS, NO_TEXTURE,
};

mod geometry_gpu;
#[cfg(test)]
mod tests;
#[cfg(test)]
#[path = "transport_tests.rs"]
mod transport_tests;
pub(crate) use ara_frame::instanced::{CheckedScene, ValidatedScene};
pub(crate) use geometry_gpu::preparation::storage_limit as geometry_storage_limit;
pub(crate) use geometry_gpu::preparation::{GeometryPreparation, GeometrySeed};
pub use geometry_gpu::{GeometryGpu, GeometryUpload, LodRange, PreparedPose};
