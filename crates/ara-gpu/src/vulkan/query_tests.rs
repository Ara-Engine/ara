use super::*;

fn timed(gpu: &mut VulkanDevice, queue: Queue, passes: usize) -> Result<TimelinePoint> {
    let mut commands = gpu.begin(queue);
    for i in 0..passes {
        commands.push(Command::BeginPass(format!("query-{i}")));
        commands.push(Command::EndPass);
    }
    gpu.submit(commands.finish(), &[])
}

#[test]
fn timestamp_pools_reuse_only_completed_submissions_and_keep_pass_results() -> Result<()> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/query-reuse");
    let mut gpu = VulkanDevice::new_raster(&[], path)?;
    for queue in [Queue::Graphics, Queue::Compute] {
        let first = timed(&mut gpu, queue, 2)?;
        let second = timed(&mut gpu, queue, 2)?;
        let first_pool = gpu
            .pending
            .iter()
            .find(|p| p.point == first)
            .map(|p| p.query);
        let second_pool = gpu
            .pending
            .iter()
            .find(|p| p.point == second)
            .map(|p| p.query);
        assert_ne!(first_pool, second_pool, "in-flight pools must be distinct");
        assert_eq!(gpu.resolve(first)?.len(), 2);
        let created = gpu.query_pool_creations;
        let reused = timed(&mut gpu, queue, 2)?;
        assert_eq!(
            gpu.query_pool_creations, created,
            "completed pools must be reused"
        );
        assert!(gpu.resolve(first).is_err());
        let times = gpu.resolve(reused)?;
        assert_eq!(
            times.iter().map(|t| t.pass.as_str()).collect::<Vec<_>>(),
            ["query-0", "query-1"]
        );
        assert!(
            times
                .iter()
                .all(|t| t.queue == queue && t.milliseconds >= 0.)
        );
        gpu.resolve(second)?;
        let mut invalid = gpu.begin(queue);
        invalid.push(Command::EndPass);
        assert!(gpu.submit(invalid.finish(), &[]).is_err());
        assert_eq!(gpu.query_pool_creations, created);
    }
    gpu.assert_clean()?;
    Ok(())
}

#[test]
fn timestamp_cache_is_bounded_and_adopts_larger_pools_without_caching_huge_ones() -> Result<()> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/query-bounds");
    let mut gpu = VulkanDevice::new_raster(&[], path)?;
    let points = (0..queries::MAX_CACHED_POOLS + 4)
        .map(|_| timed(&mut gpu, Queue::Graphics, 1))
        .collect::<Result<Vec<_>>>()?;
    for point in points {
        assert_eq!(gpu.resolve(point)?.len(), 1);
    }
    assert_eq!(gpu.query_cache.len(), queries::MAX_CACHED_POOLS);
    let created = gpu.query_pool_creations;
    for _ in 0..3 {
        let point = timed(&mut gpu, Queue::Graphics, 12)?;
        assert_eq!(gpu.resolve(point)?.len(), 12);
    }
    assert_eq!(gpu.query_pool_creations, created + 1);
    let oversized = queries::MAX_CACHED_QUERIES as usize / 2 + 1;
    for _ in 0..2 {
        let point = timed(&mut gpu, Queue::Graphics, oversized)?;
        assert_eq!(gpu.resolve(point)?.len(), oversized);
    }
    assert_eq!(gpu.query_pool_creations, created + 3);
    assert_eq!(gpu.query_cache.len(), queries::MAX_CACHED_POOLS);
    gpu.assert_clean()?;
    Ok(())
}

#[test]
fn pass_begins_exclude_previous_tail_and_frame_span_covers_both_queues() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-gpu/exclusive-timestamps");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let bytes = 16 * 1024 * 1024;
    let usage = vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST;
    let source = gpu.create_buffer(bytes, usage, true)?;
    let destination = gpu.create_buffer(bytes, usage, false)?;
    gpu.upload(source, &vec![7; bytes as usize])?;
    gpu.reset_timing_window();
    let mut commands = gpu.begin(Queue::Graphics);
    commands.push(Command::BeginPass("copy".into()));
    commands.barriers([
        Barrier {
            resource: Resource::Buffer(source),
            before: Access::HostWrite,
            after: Access::CopySource,
            handoff: None,
        },
        Barrier {
            resource: Resource::Buffer(destination),
            before: Access::Undefined,
            after: Access::CopyDestination,
            handoff: None,
        },
    ]);
    commands.push(Command::CopyBuffer {
        source,
        destination,
        size: bytes,
    });
    commands.push(Command::EndPass);
    commands.push(Command::BeginPass("empty-after-copy".into()));
    commands.barriers([Barrier {
        resource: Resource::Buffer(destination),
        before: Access::CopyDestination,
        after: Access::CopySource,
        handoff: None,
    }]);
    commands.push(Command::EndPass);
    let point = gpu.submit(commands.finish(), &[])?;
    assert!(
        gpu.timing_window().is_none(),
        "pending queries are not complete coverage"
    );
    let graphics = gpu.resolve(point)?;
    assert_eq!(graphics.len(), 2);
    assert!(
        graphics[1].start_ms >= graphics[0].end_ms,
        "the next begin must follow the previous completed tail"
    );
    for pass in &graphics {
        assert!(pass.end_ms >= pass.start_ms);
        assert!((pass.end_ms - pass.start_ms - pass.milliseconds).abs() < 1e-8);
    }
    let first_span = gpu.timing_window().expect("one queue is comparable");
    assert!(
        first_span.start_ms <= graphics[0].start_ms && first_span.end_ms >= graphics[1].end_ms,
        "{first_span:?} must contain {graphics:?}"
    );
    let mut compute = gpu.begin(Queue::Compute);
    compute.push(Command::BeginPass("compute-after-graphics".into()));
    compute.push(Command::EndPass);
    let compute = gpu.submit(compute.finish(), &[point])?;
    assert!(
        gpu.timing_window().is_none(),
        "one resolved queue is only partial coverage"
    );
    let compute = gpu.resolve(compute)?;
    if gpu.context().info.timestamp_clock.is_some() {
        assert!(
            compute[0].start_ms >= graphics[1].end_ms,
            "calibrated queues share the device epoch"
        );
        let span = gpu.timing_window().expect("calibrated multi-queue span");
        assert!(span.start_ms <= first_span.start_ms && span.end_ms >= compute[0].end_ms);
        eprintln!(
            "GPU clock {:?}: graphics {:?}, compute {:?}, span {:?}",
            gpu.context().info.timestamp_clock,
            graphics,
            compute,
            span
        );
    } else {
        assert!(
            gpu.timing_window().is_none(),
            "unrelated queue clocks must never be combined"
        );
    }
    gpu.reset_timing_window();
    assert!(gpu.timing_window().is_none());
    let commands = gpu.begin(Queue::Graphics); // Even an unlabelled list is measured.
    let point = gpu.submit(commands.finish(), &[])?;
    assert!(gpu.resolve(point)?.is_empty());
    assert!(gpu.timing_window().is_some());
    gpu.retire(Resource::Buffer(source))?;
    gpu.retire(Resource::Buffer(destination))?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
