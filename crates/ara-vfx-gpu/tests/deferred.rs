//! First-use VFX construction inside an asynchronous Scene frame. Vulkan only: the
//! atlas upload is held back with a `QueueGate`.
#![cfg(not(target_os = "macos"))]
use anyhow::Result;
use ara_gpu::{Device, GpuDevice, Queue, Resource, vk, vulkan::QueueGate};
use ara_vfx_gpu::GpuSystem;
use std::path::Path;

#[test]
fn deferred_vfx_initialization_and_failed_retries_release_their_heap_slots() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new_raster(&[], root.join("../../target/ara-vfx-gpu/deferred-cache"))?;
    let effect = ara_vfx::samples::campfire();
    let cooked = root.join("cooked/campfire");
    let baseline = gpu.descriptor_usage();
    // Block the actual atlas transfer until the constructor returns. This makes
    // the old post-submit host upload fail deterministically, without timing races.
    gpu.begin_deferred_completion()?;
    let mut gate = QueueGate::new(gpu.context(), gpu.context().scene_graphics)?;
    let mut system = GpuSystem::new(
        &mut gpu,
        &effect,
        &cooked,
        [64, 48],
        ara_vfx_gpu::LINEAR_HDR,
    );
    let advanced = match &mut system {
        Ok(system) => system.advance(&mut gpu, &Default::default()),
        Err(_) => Ok(()),
    };
    // Always release even when construction fails (Drop would too).
    gate.open()?;
    let mut completion = gpu.end_deferred_completion()?;
    gpu.wait_completion(&mut completion)?;
    drop(gate);
    system?.retire(&mut gpu)?;
    advanced?;
    gpu.collect()?;
    assert_eq!(gpu.descriptor_usage(), baseline);

    let mut pressure = Vec::new();
    let capacity = gpu.info().storage_buffer_capacity as usize;
    while gpu.descriptor_usage().storage_buffers < capacity - 2 {
        let buffer = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        gpu.storage_buffer(buffer)?;
        pressure.push(buffer);
    }
    let occupied = gpu.descriptor_usage();
    for _ in 0..8 {
        assert!(
            GpuSystem::new(
                &mut gpu,
                &effect,
                &cooked,
                [64, 48],
                ara_vfx_gpu::LINEAR_HDR
            )
            .is_err()
        );
        gpu.collect()?;
        assert_eq!(gpu.descriptor_usage(), occupied);
    }
    for buffer in pressure {
        gpu.retire(Resource::Buffer(buffer))?;
    }
    gpu.collect()?;
    assert_eq!(gpu.descriptor_usage(), baseline);
    GpuSystem::new(
        &mut gpu,
        &effect,
        &cooked,
        [64, 48],
        ara_vfx_gpu::LINEAR_HDR,
    )?
    .retire(&mut gpu)?;
    gpu.assert_clean()?;
    // Keep the dedicated compute path available to ordinary synchronous clients.
    let point = gpu.submit(gpu.begin(Queue::Compute).finish(), &[])?;
    gpu.resolve(point)?;
    Ok(())
}
