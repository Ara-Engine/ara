//! First-use VFX construction inside an asynchronous Scene frame.
use anyhow::Result;
use ara_gpu::vk::TaggedStructure;
use ara_gpu::{Device, Queue, Resource, vk, vulkan::VulkanDevice};
use ara_vfx_gpu::GpuSystem;
use std::path::Path;

#[test]
fn deferred_vfx_initialization_and_failed_retries_release_their_heap_slots() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu =
        VulkanDevice::new_raster(&[], root.join("../../target/ara-vfx-gpu/deferred-cache"))?;
    let effect = ara_vfx::samples::campfire();
    let cooked = root.join("cooked/campfire");
    let baseline = gpu.descriptor_usage();
    // Block the actual atlas transfer until the constructor returns. This makes
    // the old post-submit host upload fail deterministically, without timing races.
    let device = gpu.context().device.clone();
    let mut kind =
        vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
    // SAFETY: Test-owned semaphore, signalled and destroyed after its only wait.
    let gate = unsafe {
        device.create_semaphore(&vk::SemaphoreCreateInfo::default().push(&mut kind), None)?
    };
    gpu.begin_deferred_completion()?;
    // SAFETY: The owner thread submits an empty wait before the atlas transfer.
    unsafe {
        device.queue_submit2(
            gpu.context().scene_graphics,
            &[vk::SubmitInfo2::default().wait_semaphore_infos(&[
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(gate)
                    .value(1)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
            ])],
            vk::Fence::null(),
        )?;
    }
    let mut system = GpuSystem::new(&mut gpu, &effect, &cooked, [64, 48]);
    let advanced = match &mut system {
        Ok(system) => system.advance(&mut gpu, &Default::default()),
        Err(_) => Ok(()),
    };
    // Always release even when construction fails: an assertion must not hang Drop.
    // SAFETY: Sole signal releases the test-owned queue wait.
    unsafe {
        device.signal_semaphore(&vk::SemaphoreSignalInfo::default().semaphore(gate).value(1))?;
    }
    let mut completion = gpu.end_deferred_completion()?;
    gpu.wait_completion(&mut completion)?;
    // SAFETY: Completion drained all submissions after the semaphore wait.
    unsafe {
        device.destroy_semaphore(gate, None);
    }
    system?.retire(&mut gpu)?;
    advanced?;
    gpu.collect()?;
    assert_eq!(gpu.descriptor_usage(), baseline);

    let mut pressure = Vec::new();
    let capacity = gpu.context().info.storage_buffer_capacity as usize;
    while gpu.descriptor_usage().storage_buffers < capacity - 2 {
        let buffer = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        gpu.storage_buffer(buffer)?;
        pressure.push(buffer);
    }
    let occupied = gpu.descriptor_usage();
    for _ in 0..8 {
        assert!(GpuSystem::new(&mut gpu, &effect, &cooked, [64, 48]).is_err());
        gpu.collect()?;
        assert_eq!(gpu.descriptor_usage(), occupied);
    }
    for buffer in pressure {
        gpu.retire(Resource::Buffer(buffer))?;
    }
    gpu.collect()?;
    assert_eq!(gpu.descriptor_usage(), baseline);
    GpuSystem::new(&mut gpu, &effect, &cooked, [64, 48])?.retire(&mut gpu)?;
    gpu.assert_clean()?;
    // Keep the dedicated compute path available to ordinary synchronous clients.
    let point = gpu.submit(gpu.begin(Queue::Compute).finish(), &[])?;
    gpu.resolve(point)?;
    Ok(())
}
