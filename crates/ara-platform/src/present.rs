//! Native window presentation, selected per backend like `ara_gpu::GpuDevice`: Vulkan
//! swapchains everywhere except macOS, where an SDL Metal view's CAMetalLayer presents
//! through `ara_gpu::metal::present`. Both types share the backend-neutral surface
//! (`create`, `create_with_output`, `present_image`, `present_encoded`, `retire`,
//! `output_settings`, `surface_format`); the Vulkan type keeps its native API for the
//! editor's raw UI presentation.
#[cfg(target_os = "macos")]
mod metal;
#[cfg(not(target_os = "macos"))]
mod vulkan;

#[cfg(target_os = "macos")]
pub use metal::Present;
#[cfg(not(target_os = "macos"))]
pub use vulkan::Present;
