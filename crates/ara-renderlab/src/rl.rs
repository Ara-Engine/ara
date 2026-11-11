//! The Renderlab engine this target draws with. Elsewhere it is the wgpu engine, on
//! Renderlab's own Vulkan device; on Apple it is the ara-gpu backend's, which renders on
//! the presenter's device (Metal). The scene mapping (`scene`, `terrain`, `splat`,
//! `effects`, `decals`) makes the same calls on either.
#[cfg(target_vendor = "apple")]
pub(crate) use renderlab::backend_ara::engine::Engine;
#[cfg(not(target_vendor = "apple"))]
pub(crate) use renderlab::engine::Engine;
