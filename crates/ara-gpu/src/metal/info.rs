//! The portable [`DeviceInfo`] for a Metal device, so consumers read one capability
//! record on both backends (`GpuDevice::info`). Vulkan-only fields get the values
//! that describe this backend honestly: no optional tiers it has not enabled, one
//! GPU clock for both queues, unified memory.
use super::abi;
use crate::shared::DeviceIdentity;
use crate::vulkan::DeviceInfo;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSProcessInfo;
use objc2_metal::*;

/// Apple's PCI vendor ID.
const APPLE_VENDOR: u32 = 0x106b;
/// Largest 2D texture extent on Apple silicon GPUs.
const MAX_IMAGE_DIMENSION: u32 = 16_384;

/// Capabilities recorded once at device creation.
pub(super) fn device_info(device: &ProtocolObject<dyn MTLDevice>) -> DeviceInfo {
    let frequency = device.queryTimestampFrequency().max(1);
    DeviceInfo {
        name: device.name().to_string(),
        vendor_id: APPLE_VENDOR,
        device_id: 0,
        discrete: false,
        // Unified memory: the working set Metal recommends for this process.
        device_local_mib: device.recommendedMaxWorkingSetSize() / (1024 * 1024),
        ray_tracing: false,
        texture_compression_bc: device.supportsBCTextureCompression(),
        sampled_image_capacity: abi::TEXTURES,
        storage_buffer_capacity: abi::BUFFERS,
        mesh_shading: false,
        mesh_dispatch: [0; 4],
        task_dispatch: [0; 4],
        geometry_shader: false,
        hdr_metadata: false,
        buffer_int64_atomics: false,
        shader_integer_dot_product: false,
        packed_integer_dot_accelerated: false,
        max_image_dimension: MAX_IMAGE_DIMENSION,
        api_version: format!(
            "Metal 4 ({})",
            NSProcessInfo::processInfo().operatingSystemVersionString()
        ),
        driver_version: 0,
        graphics_family: 0,
        compute_family: 1,
        graphics_queue_flags: "MTL4CommandQueue (render, compute, blit)".into(),
        compute_queue_flags: "MTL4CommandQueue (compute, blit)".into(),
        descriptor_heap_available: false,
        timestamp_period_ns: (1e9 / frequency as f64) as f32,
        timestamp_valid_bits: [64, 64],
        // Both queues' timestamp heaps read one GPU clock.
        timestamp_clock: Some("MTL4 timestamp heaps"),
        // The persistent cache is an MTL4 archive keyed by GPU and OS build.
        pipeline_cache_uuid: Vec::new(),
        // Shared surfaces are IOSurfaces ([`super::external`]).
        external_memory: true,
        identity: identity(device),
    }
}

/// The identity both shared-surface sides compare; Apple silicon Macs have one GPU
/// per registry ID.
pub(super) fn identity(device: &ProtocolObject<dyn MTLDevice>) -> DeviceIdentity {
    let mut device_uuid = [0; 16];
    device_uuid[..8].copy_from_slice(&device.registryID().to_le_bytes());
    DeviceIdentity {
        device_uuid,
        driver_uuid: [0; 16],
        luid: None,
    }
}
