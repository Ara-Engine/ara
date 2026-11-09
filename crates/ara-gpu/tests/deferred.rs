//! A deliberately blocked Scene must not block the native UI queue or the CPU.
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource, vk,
    vulkan::{Command, QueueGate, VulkanDevice},
};

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn current_queue_scope_preserves_phase_timings_and_waits_only_at_its_tail() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-gpu/deferred-frame");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let submits = gpu.submission_stats();
    gpu.begin_deferred_completion_on_current_queue()?;
    let mut points = Vec::new();
    for phase in ["view", "feature", "post"] {
        let mut commands = gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass(phase.into()));
        commands.push(Command::EndPass);
        let point = gpu.submit(commands.finish(), &[])?;
        assert!(gpu.resolve(point)?.is_empty());
        points.push(point);
        assert_eq!(gpu.deferred_submission_count(), points.len());
    }
    assert_eq!(
        gpu.submission_stats(),
        submits,
        "recording has not submitted a partial frame"
    );
    assert!(
        gpu.wait(points[0]).is_err(),
        "a staged point cannot block the host"
    );
    let mut completion = gpu.end_deferred_completion()?;
    let submitted = gpu.submission_stats();
    assert_eq!(submitted.queue_submissions - submits.queue_submissions, 1);
    assert_eq!(submitted.command_lists - submits.command_lists, 3);
    let before = gpu.context().host_wait_stats();
    let mut phases = Vec::new();
    gpu.wait_completion_with(&mut completion, |point, passes| {
        assert_eq!(point, points[phases.len()]);
        assert_eq!(passes.len(), 1);
        phases.push(passes.into_iter().next().expect("one phase").pass);
    })?;
    assert_eq!(phases, ["view", "feature", "post"]);
    assert_eq!(gpu.context().host_wait_stats().calls - before.calls, 1);
    assert!(completion.is_resolved());
    gpu.assert_clean()?;
    Ok(())
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn deferred_frame_polls_without_waiting_and_orders_constant_writes() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/gpu/deferred")
        .join(std::process::id().to_string());
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let usage = vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST;
    let source = gpu.create_buffer(16, usage, true)?;
    let before = gpu.create_buffer(16, usage, true)?;
    let after = gpu.create_buffer(16, usage, true)?;
    gpu.upload(source, &[1; 16])?;
    gpu.upload(before, &[0; 16])?;
    gpu.upload(after, &[0; 16])?;
    gpu.begin_deferred_completion()?;
    assert!(gpu.begin_deferred_completion().is_err());
    // The gate deliberately stops compute until after the nonblocking assertions.
    let mut gate = QueueGate::new(gpu.context(), gpu.context().compute)?;
    let mut compute = gpu.begin(Queue::Compute);
    compute.push(Command::BeginPass("Deferred.Gate".into()));
    compute.push(Command::EndPass);
    let gate_point = gpu.submit(compute.finish(), &[])?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.barriers([
        Barrier {
            resource: Resource::Buffer(source),
            before: Access::HostWrite,
            after: Access::CopySource,
            handoff: None,
        },
        Barrier {
            resource: Resource::Buffer(before),
            before: Access::HostWrite,
            after: Access::CopyDestination,
            handoff: None,
        },
    ]);
    commands.push(Command::BeginPass("Deferred.Before".into()));
    commands.push(Command::CopyBuffer {
        source,
        destination: before,
        size: 16,
    });
    commands.barriers([Barrier {
        resource: Resource::Buffer(before),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    commands.push(Command::EndPass);
    let first = gpu.submit(commands.finish(), &[gate_point])?;
    assert!(
        gpu.resolve(first)?.is_empty(),
        "deferred resolve must not wait for the gate"
    );
    gpu.upload_queued_range(source, 4, &[2; 8])?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.barriers([Barrier {
        resource: Resource::Buffer(after),
        before: Access::HostWrite,
        after: Access::CopyDestination,
        handoff: None,
    }]);
    commands.push(Command::BeginPass("Deferred.After".into()));
    commands.push(Command::CopyBuffer {
        source,
        destination: after,
        size: 16,
    });
    commands.barriers([Barrier {
        resource: Resource::Buffer(after),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    commands.push(Command::EndPass);
    let point = gpu.submit(commands.finish(), &[])?;
    assert!(gpu.resolve(point)?.is_empty());
    let mut completion = gpu.end_deferred_completion()?;
    assert!(gpu.poll_completion(&mut completion)?.is_none());
    // The next frame records while this one runs; one more is refused until the
    // oldest is consumed: FRAMES_IN_FLIGHT bounded frames, no queue growth.
    gpu.begin_deferred_completion()?;
    let mut next = gpu.end_deferred_completion()?;
    assert_eq!(ara_gpu::FRAMES_IN_FLIGHT, 2);
    assert!(
        gpu.begin_deferred_completion().is_err(),
        "bounded frames in flight, no queue growth"
    );
    if gpu.context().scene_graphics != gpu.context().graphics {
        // SAFETY: This empty native UI submit uses its independent queue. The
        // fence is idle, and is destroyed only after it signals.
        unsafe {
            let fence = gpu
                .context()
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)?;
            gpu.context()
                .device
                .queue_submit2(gpu.context().graphics, &[], fence)?;
            let result = gpu
                .context()
                .device
                .wait_for_fences(&[fence], true, 1_000_000_000);
            if result.is_err() {
                gate.open()?;
                gpu.context()
                    .device
                    .wait_for_fences(&[fence], true, u64::MAX)?;
            }
            gpu.context().device.destroy_fence(fence, None);
            result?;
        }
    }
    gate.open()?;
    gpu.wait(point)?;
    let timings = gpu
        .poll_completion(&mut completion)?
        .expect("completed frame");
    assert!(
        gpu.poll_completion(&mut completion).is_err(),
        "resolved tokens cannot release a newer frame"
    );
    assert!(
        gpu.poll_completion(&mut next)?
            .expect("empty frame")
            .is_empty()
    );
    assert!(timings.iter().any(|p| p.pass == "Deferred.Before"));
    assert!(timings.iter().any(|p| p.pass == "Deferred.After"));
    assert_eq!(gpu.read(before)?, [1; 16]);
    assert_eq!(
        gpu.read(after)?,
        [1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1]
    );
    gpu.assert_clean()?;
    Ok(())
}
