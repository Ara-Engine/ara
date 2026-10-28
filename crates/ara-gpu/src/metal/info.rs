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
/// Largest 2D texture extent on Apple silicon GPUs; image creation enforces it.
pub(super) const MAX_IMAGE_DIMENSION: u32 = 16_384;

/// The RT tier (D29): ray queries on GPUs with hardware ray tracing, Apple9 (M3) and
/// later. Earlier Apple GPUs trace in software and take the raster fallback.
pub(super) fn hardware_ray_tracing(device: &ProtocolObject<dyn MTLDevice>) -> bool {
    device.supportsRaytracing() && device.supportsFamily(MTLGPUFamily::Apple9)
}

/// 64-bit buffer atomics: unsigned min/max on `ulong` in device memory, which Metal
/// Shader Converter maps DXIL's `InterlockedMax64`/`InterlockedMin64` onto, on Apple9
/// (M3) and later. Other 64-bit atomic operations have no Metal equivalent; Ara's
/// shaders use only max (the geometry visibility buffer).
fn buffer_int64_atomics(device: &ProtocolObject<dyn MTLDevice>) -> bool {
    device.supportsFamily(MTLGPUFamily::Apple9)
}

/// Capabilities recorded once at device creation; `ray_tracing` is set by the
/// RT-tier constructor ([`hardware_ray_tracing`]).
pub(super) fn device_info(device: &ProtocolObject<dyn MTLDevice>) -> DeviceInfo {
    let frequency = device.queryTimestampFrequency().max(1);
    DeviceInfo {
        name: device.name().to_string(),
        vendor_id: APPLE_VENDOR,
        device_id: 0,
        discrete: false,
        // Unified memory is device-local on Apple GPUs: plan with the working set
        // Metal recommends for this process.
        device_local_mib: device.recommendedMaxWorkingSetSize() / (1024 * 1024),
        unified_device_local: device.hasUnifiedMemory(),
        ray_tracing: false,
        texture_compression_bc: device.supportsBCTextureCompression(),
        sampled_image_capacity: abi::TEXTURES,
        storage_buffer_capacity: abi::BUFFERS,
        mesh_shading: false,
        mesh_dispatch: [0; 4],
        task_dispatch: [0; 4],
        geometry_shader: false,
        hdr_metadata: false,
        buffer_int64_atomics: buffer_int64_atomics(device),
        shader_integer_dot_product: false,
        packed_integer_dot_accelerated: false,
        max_image_dimension: MAX_IMAGE_DIMENSION,
        max_storage_buffer_range: u32::try_from(device.maxBufferLength()).unwrap_or(u32::MAX),
        // Every Metal 4 GPU has SIMD-group functions in compute and fragment shaders,
        // which Metal Shader Converter maps wave operations onto.
        wave_ops: true,
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
        // Metal queues expose no priority control; the OS default matches Vulkan medium.
        graphics_global_priority: "medium",
        global_priority_extension: None,
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
