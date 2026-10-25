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
