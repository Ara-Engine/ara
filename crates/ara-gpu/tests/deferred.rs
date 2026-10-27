//! A deliberately blocked Scene must not block the native UI queue or the CPU.
use ara_gpu::vk::TaggedStructure;
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource, vk,
    vulkan::{Command, VulkanDevice},
};

struct Gate {
    device: ash::Device,
    semaphore: vk::Semaphore,
    opened: bool,
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn current_queue_scope_preserves_phase_timings_and_waits_only_at_its_tail() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-gpu/deferred-frame");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
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
    let mut completion = gpu.end_deferred_completion()?;
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
impl Gate {
    fn open(&mut self) -> anyhow::Result<()> {
        if !self.opened {
            // SAFETY: This test owns the only host signal for this timeline.
            unsafe {
                self.device.signal_semaphore(
                    &vk::SemaphoreSignalInfo::default()
                        .semaphore(self.semaphore)
                        .value(1),
                )?;
            }
            self.opened = true;
        }
        Ok(())
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        let _ = self.open();
        // SAFETY: Signalling releases the test's blocked queues, including on an
        // early assertion failure; idle precedes destruction of their wait object.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_semaphore(self.semaphore, None);
        }
    }
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
    // SAFETY: The new semaphore belongs to this device. Gate owns its lifetime
    // through all pending waits, and always signals it before teardown.
    let mut gate = unsafe {
        let mut kind =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        Gate {
            device: gpu.context().device.clone(),
            semaphore: gpu
                .context()
                .device
                .create_semaphore(&vk::SemaphoreCreateInfo::default().push(&mut kind), None)?,
            opened: false,
        }
    };
    gpu.begin_deferred_completion()?;
    assert!(gpu.begin_deferred_completion().is_err());
    // SAFETY: Only this owner thread submits to these queues. The host-signalled
    // gate deliberately stops compute until after the nonblocking assertions.
    unsafe {
        gpu.context().device.queue_submit2(
            gpu.context().compute,
            &[vk::SubmitInfo2::default().wait_semaphore_infos(&[
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(gate.semaphore)
                    .value(1)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
            ])],
            vk::Fence::null(),
        )?;
    }
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
    assert!(
        gpu.begin_deferred_completion().is_err(),
        "one bounded frame, no queue growth"
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
