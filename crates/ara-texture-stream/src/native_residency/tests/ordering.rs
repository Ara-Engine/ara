use super::*;
use ara_gpu::vk::TaggedStructure;

struct Gate {
    device: ash::Device,
    semaphore: vk::Semaphore,
}

impl Gate {
    fn new(gpu: &GpuDevice) -> Result<Self> {
        let device = gpu.context().device.clone();
        // SAFETY: The timeline lives until the queue has drained in Drop; the
        // creating device is retained by the test and outlives this guard.
        let semaphore = unsafe {
            let mut timeline =
                vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
            device.create_semaphore(
                &vk::SemaphoreCreateInfo::default().push(&mut timeline),
                None,
            )?
        };
        let result = Self { device, semaphore };
        // SAFETY: This test serializes the queue. The wait is released even when
        // an assertion fails, before its device is destroyed.
        unsafe {
            let waits = [vk::SemaphoreSubmitInfo::default()
                .semaphore(semaphore)
                .value(1)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            result.device.queue_submit2(
                gpu.context().graphics,
                &[vk::SubmitInfo2::default().wait_semaphore_infos(&waits)],
                vk::Fence::null(),
            )?;
        }
        Ok(result)
    }
    fn open(&self) -> Result<()> {
        // SAFETY: Host signal of this test-owned timeline's only wait.
        unsafe {
            self.device.signal_semaphore(
                &vk::SemaphoreSignalInfo::default()
                    .semaphore(self.semaphore)
                    .value(1),
            )?;
        }
        Ok(())
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        // SAFETY: Drop releases the test wait before waiting for and destroying
        // its semaphore; this guard is always dropped before the GPU owner.
        unsafe {
            if self
                .device
                .get_semaphore_counter_value(self.semaphore)
                .is_ok_and(|value| value == 0)
            {
                let _ = self.open();
            }
            let _ = self.device.device_wait_idle();
            self.device.destroy_semaphore(self.semaphore, None);
        }
    }
}

#[test]
fn preallocated_table_is_gpu_ordered_after_an_earlier_inflight_submission() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-texture-stream/prepared-table-ordering");
    let mut gpu = GpuDevice::new_raster(&[], root)?;
    let mut set = bank(1024, 1024);
    set.register([1; 32], source([4, 4], 1, 200))?;
    let start = Instant::now();
    while !set.preallocate(&mut gpu, None)? {
        ensure!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "texture allocation timeout"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let gate = Gate::new(&gpu)?;
    gpu.begin_deferred_completion_on_current_queue()?;
    let mut earlier = gpu.begin(Queue::Graphics);
    earlier.push(Command::BeginPass("Test.EarlierGeometry".into()));
    earlier.push(Command::EndPass);
    let point = gpu.submit(earlier.finish(), &[])?;
    assert!(
        gpu.completed()?[point.queue.index()] < point.value,
        "physical GPU gate holds the earlier submission"
    );
    let waits = gpu.host_wait_stats().calls;
    assert_eq!(set.initialize(&mut gpu)?.textures, 1);
    assert_eq!(
        gpu.host_wait_stats().calls,
        waits,
        "publication must not host-wait"
    );
    let mut completion = gpu.end_deferred_completion()?;
    gate.open()?;
    gpu.wait_completion(&mut completion)?;
    assert_eq!(
        blocks(&mut gpu, set.entries[0].resident.expect("published tail")),
        [200, 200, 0, 0, 0, 0, 0, 0]
    );
    set.destroy(&mut gpu)?;
    drop(gate);
    gpu.assert_clean()?;
    Ok(())
}
