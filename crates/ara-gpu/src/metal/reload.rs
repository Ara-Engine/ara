//! The shader reload surface of [`crate::vulkan::VulkanDevice`] on Metal. Background
//! cohort reload (`ARA_SHADER_DIR`) is not on Metal yet: the epoch never advances,
//! like a Vulkan device without a development override, and a reload request is
//! reported through `ShaderStatus::last_error` instead of being silently dropped.
use super::MetalDevice;
use crate::vulkan::ShaderStatus;

impl MetalDevice {
    /// Record that a reload was asked for; Metal keeps the loaded pipelines.
    pub fn request_shader_reload(&mut self) {
        self.shader_status.last_error =
            Some("shader hot reload is not available on the Metal backend yet".into());
    }
    /// Active shader provenance; no cohorts are tracked on Metal yet.
    pub fn shader_status(&self) -> ShaderStatus {
        self.shader_status.clone()
    }
    /// Publication counter for temporal-history invalidation; constant on Metal.
    pub fn shader_epoch(&self) -> u64 {
        self.shader_status.epoch
    }
}
