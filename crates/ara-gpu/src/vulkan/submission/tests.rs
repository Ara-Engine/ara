use super::*;

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn rejected_middle_helper_still_flushes_and_retires_earlier_recorded_work() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-gpu/batch-rejection");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let baseline = gpu.descriptor_usage();
    for _ in 0..8 {
        let buffer = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        gpu.storage_buffer(buffer)?;
        gpu.begin_deferred_completion_on_current_queue()?;
        let commands = gpu.begin(Queue::Graphics);
        let point = gpu.submit(commands.finish(), &[])?;
        let mut invalid = gpu.begin(Queue::Graphics);
        invalid.push(Command::EndPass);
        assert!(gpu.submit(invalid.finish(), &[]).is_err());
        gpu.retire(Resource::Buffer(buffer))?;
        gpu.collect()?;
        assert!(gpu.descriptor_usage().storage_buffers > baseline.storage_buffers);
        let mut completion = gpu.end_deferred_completion()?;
        gpu.wait_completion(&mut completion)?;
        assert!(
            gpu.resolve(point).is_err(),
            "completion consumed every original query group"
        );
        assert_eq!(gpu.descriptor_usage(), baseline);
    }
    gpu.assert_clean()
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn native_submission_failure_drains_recording_and_rejects_retry_without_heap_growth() -> Result<()>
{
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/batch-failure");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let baseline = gpu.descriptor_usage();
    let buffer = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    gpu.storage_buffer(buffer)?;
    gpu.begin_deferred_completion_on_current_queue()?;
    let commands = gpu.begin(Queue::Graphics);
    gpu.submit(commands.finish(), &[])?;
    gpu.retire(Resource::Buffer(buffer))?;
    gpu.fail_batch = true;
    let mut completion = gpu.end_deferred_completion()?;
    assert!(gpu.wait_completion(&mut completion).is_err());
    assert!(completion.is_resolved());
    assert!(gpu.pending.is_empty());
    assert!(gpu.batch.entries.is_empty());
    assert_eq!(gpu.descriptor_usage(), baseline);
    for _ in 0..8 {
        assert!(gpu.begin_deferred_completion().is_err());
        gpu.collect()?;
        assert_eq!(gpu.descriptor_usage(), baseline);
    }
    gpu.assert_clean()
}

/// A bounded completion wait never consumes a frame it gave up on: false leaves it
/// pending for a later wait (an event pump's retry); true consumes it once. Two
/// frames may be pending at once, the newer recorded while the older runs.
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn bounded_completion_waits_leave_unfinished_frames_pending() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-gpu/bounded-completion");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let mut frames = Vec::new();
    for _ in 0..crate::FRAMES_IN_FLIGHT {
        gpu.begin_deferred_completion_on_current_queue()?;
        gpu.submit(gpu.begin(Queue::Graphics).finish(), &[])?;
        frames.push(gpu.end_deferred_completion()?);
    }
    assert!(
        gpu.begin_deferred_completion_on_current_queue().is_err(),
        "a third frame waits for the oldest"
    );
    for frame in &mut frames {
        // Zero time may or may not suffice; either way the contract holds.
        if !gpu.wait_completion_for(frame, std::time::Duration::ZERO, |_, _| {})? {
            assert!(!frame.is_resolved(), "a timed-out wait consumed the frame");
            let done =
                gpu.wait_completion_for(frame, std::time::Duration::from_secs(5), |_, _| {})?;
            assert!(done, "an idle device completes the frame");
        }
        assert!(frame.is_resolved());
    }
    gpu.assert_clean()?;
    Ok(())
}
