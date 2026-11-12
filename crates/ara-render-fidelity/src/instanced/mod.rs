//! The GPU geometry pool the fidelity view traces: model-space vertex, index and BLAS
//! storage for a scene's geometries, their poses and skins. The CPU records live in
//! `ara-frame` and are re-exported here.
pub(crate) use ara_frame::instanced::ValidatedScene;
pub use ara_frame::instanced::{Geometry, Lod, Material, Scene, Vertex};

mod geometry_gpu;
pub use geometry_gpu::{GeometryGpu, GeometryUpload, LodRange, PreparedPose};
