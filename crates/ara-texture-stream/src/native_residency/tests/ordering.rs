use super::*;
use ara_gpu::vulkan::QueueGate;

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
    let mut gate = QueueGate::new(gpu.context(), gpu.context().graphics)?;
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
