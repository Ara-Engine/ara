use super::*;
use crate::instanced::GeometryPreparation;
use std::time::{Duration, Instant};

fn wait(gpu: &mut VulkanDevice, plan: &mut GeometryPreparation) -> Result<()> {
    let start = Instant::now();
    while !plan.preallocate(gpu)? {
        ensure!(
            start.elapsed() < Duration::from_secs(5),
            "geometry worker timeout"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
fn worker_prepared_geometry_reuses_static_sources_and_adopts_current_skin_bytes() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/prepared-geometry");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let original = fixture();
    let pool = GeometryGpu::new(&mut gpu, &original, false)?;
    let baseline = gpu.descriptor_usage();
    for _ in 0..8 {
        let seed = pool.preparation_seed();
        let scene = fixture();
        let worker = std::thread::spawn(move || {
            GeometryPreparation::new(scene.validated()?, 4096, Some(&seed))
        });
        let mut rejected = worker.join().expect("CPU geometry worker")?;
        wait(&mut gpu, &mut rejected)?;
        rejected.destroy(&mut gpu)?;
        gpu.collect()?;
        assert_eq!(gpu.descriptor_usage(), baseline);
    }
    let seed = pool.preparation_seed();
    let worker = std::thread::spawn(move || -> Result<_> {
        let mut scene = fixture();
        scene.geometry.reverse();
        for vertex in &mut scene.geometry[0].vertices {
            vertex.position[0] += 0.25;
        }
        for bound in &mut scene.geometry[0].bounds {
            bound.x += 0.25;
        }
        scene.geometry[0].revision += 1;
        let mut added = fixture().geometry.remove(0);
        added.key = [7; 32];
        scene.geometry.push(added);
        let plan = GeometryPreparation::new(scene.validated()?, 4096, Some(&seed))?;
        Ok((scene, plan))
    });
    let (scene, mut plan) = worker.join().expect("CPU geometry worker")?;
    assert!(plan.source_matches(Some(&pool)));
    assert!(!plan.source_matches(None));
    let waits = gpu.context().host_wait_stats().calls;
    wait(&mut gpu, &mut plan)?;
    assert_eq!(gpu.context().host_wait_stats().calls, waits);
    assert_eq!(
        gpu.descriptor_usage(),
        baseline,
        "allocation does not bind descriptors"
    );
    let candidate = plan.publish(&mut gpu)?;
    assert_eq!(candidate.stats().reused_geometries, 2);
    assert_eq!(
        candidate.stats().uploaded_bytes + candidate.stats().reused_bytes,
        candidate.stats().bytes
    );
    reuse::verify_static(&mut gpu, &candidate, &scene)?;
    for buffer in candidate.dynamic.expect("skin slots") {
        assert_eq!(
            gpu.read(buffer)?,
            bytemuck::cast_slice::<_, u8>(&scene.geometry[0].vertices)
        );
    }
    candidate.destroy(&mut gpu)?;
    assert_eq!(gpu.descriptor_usage(), baseline);
    reuse::verify_static(&mut gpu, &pool, &original)?;
    pool.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}

#[test]
#[ignore = "paired geometry publication benchmark; run under ara-dev gpu"]
fn measure_worker_prepared_geometry_publication() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/geometry-publication-cost");
    let mut samples = Vec::new();
    for repeat in 0..2 {
        for asynchronous in [false, true] {
            let mut gpu = VulkanDevice::new_raster(&[], root.join("gpu"))?;
            let mut scene = fixture();
            scene.geometry.truncate(1);
            scene.geometry[0].vertices = vec![scene.geometry[0].vertices[0]; 1 << 20];
            let begin = Instant::now();
            let mut preparation_owner_ms = 0.;
            let mut prepared = None;
            if asynchronous {
                let worker = std::thread::spawn(move || -> Result<_> {
                    let plan = GeometryPreparation::new(scene.validated()?, 1 << 30, None)?;
                    Ok((scene, plan))
                });
                while !worker.is_finished() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                let start = Instant::now();
                let (returned, mut plan) = worker.join().expect("benchmark content worker")?;
                preparation_owner_ms += start.elapsed().as_secs_f64() * 1000.;
                scene = returned;
                loop {
                    let start = Instant::now();
                    let ready = plan.preallocate(&mut gpu)?;
                    preparation_owner_ms += start.elapsed().as_secs_f64() * 1000.;
                    if ready {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                prepared = Some(plan);
            }
            let preparation_wall_ms = begin.elapsed().as_secs_f64() * 1000.;
            gpu.begin_deferred_completion_on_current_queue()?;
            let start = Instant::now();
            let result = match prepared {
                Some(plan) => plan.publish(&mut gpu),
                None => GeometryGpu::new(&mut gpu, &scene, false),
            };
            let publish_owner_ms = start.elapsed().as_secs_f64() * 1000.;
            let mut completion = gpu.end_deferred_completion()?;
            gpu.wait_completion(&mut completion)?;
            let pool = result?;
            samples.push(serde_json::json!({
                "repeat": repeat, "asynchronous": asynchronous,
                "vertices": scene.geometry[0].vertices.len(),
                "payload_bytes": pool.stats().bytes,
                "publish_owner_ms": publish_owner_ms,
                "preparation_owner_ms": preparation_owner_ms,
                "preparation_wall_ms": preparation_wall_ms,
                "through_upload_ms": begin.elapsed().as_secs_f64() * 1000.,
            }));
            pool.destroy(&mut gpu)?;
            gpu.assert_clean()?;
        }
    }
    std::fs::create_dir_all(&root)?;
    let text = serde_json::to_string_pretty(&serde_json::json!({"samples": samples}))?;
    std::fs::write(root.join("paired.json"), &text)?;
    println!("{text}");
    Ok(())
}
