//! Deterministic landscape captures and serialized diagnostic GPU measurements.
use anyhow::Result;
use ara_gpu::GpuDevice;
use ara_landscape::{Landscape, View, sample, timing_report};
use std::{path::Path, time::Instant};
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("captures");
    std::fs::create_dir_all(&output)?;
    std::fs::create_dir_all(root.join("measurements"))?;
    let start = Instant::now();
    let data = sample::landscape(Some(&root.join("cache")))?;
    println!(
        "extracted {} instances, {} samples in {:.2}s",
        data.instances.len(),
        data.samples.len(),
        start.elapsed().as_secs_f64()
    );
    let mut gpu = GpuDevice::new(&[], root.join("cache/gpu"))?;
    let mut renderer = Landscape::new(&mut gpu, &data, [2560, 1440], &root.join("cooked"))?;
    let content = blake3::hash(bytemuck::cast_slice(&data.samples))
        .to_hex()
        .to_string();
    let instance_content = blake3::hash(bytemuck::cast_slice(&data.instances))
        .to_hex()
        .to_string();
    drop(data);
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let cache = root.join("cache");
    let loader = std::thread::spawn(move || -> Result<()> {
        use ara_terrain::cache::TileSource;
        let mut db = ara_schema::SchemaDb::new();
        ara_terrain::register(&mut db).map_err(|d| anyhow::anyhow!(d.message))?;
        let text = std::fs::read_to_string(cache.join("landscape.terrain"))?;
        let read = ara_doc::read(&text, "landscape.terrain", &db, None);
        let terrain = ara_terrain::Terrain::from_document(
            &read
                .doc
                .ok_or_else(|| anyhow::anyhow!("missing streaming manifest"))?,
        )
        .map_err(|d| anyhow::anyhow!(d.message))?;
        let source = ara_terrain::cache::FileCache::new(cache);
        for z in 14..18 {
            for x in 22..26 {
                let coord = ara_terrain::TileCoord { x, z };
                let hash = terrain
                    .tiles
                    .values()
                    .find(|t| t.coord == coord)
                    .and_then(|t| t.base.as_ref())
                    .ok_or_else(|| anyhow::anyhow!("streaming source tile absent"))?;
                let tile = source.load(hash).map_err(|d| anyhow::anyhow!(d.message))?;
                if tx.send((coord, tile)).is_err() {
                    return Ok(());
                }
            }
        }
        Ok(())
    });
    let mut states = serde_json::Map::new();
    let mut streamed = 0;
    for (name, view) in [("noon", View::noon()), ("dusk", View::dusk())] {
        let mut samples = Vec::new();
        let mut cpu = Vec::new();
        for frame in 0..40 {
            let start = Instant::now();
            if let Ok((coord, tile)) = rx.try_recv() {
                anyhow::ensure!(
                    renderer.queue_tile(&gpu, coord, &tile, true)?,
                    "tile upload budget unexpectedly exceeded"
                );
                streamed += 1;
            }
            renderer.update(&gpu, &view)?;
            let list = renderer.record(&gpu)?;
            let point = gpu.submit(list, &[])?;
            cpu.push(start.elapsed().as_secs_f64() * 1000.);
            let timings = gpu.resolve(point)?;
            if frame >= 8 {
                samples.push(timings);
            }
        }
        renderer.capture(&mut gpu, &output.join(format!("{name}.png")))?;
        // Capture twice after identical fixed-tick frames and compare actual pixels.
        renderer.update(&gpu, &view)?;
        let list = renderer.record(&gpu)?;
        let point = gpu.submit(list, &[])?;
        gpu.resolve(point)?;
        let repeated = output.join(format!("{name}-repeat.png"));
        renderer.capture(&mut gpu, &repeated)?;
        let a = image::open(output.join(format!("{name}.png")))?.to_rgba8();
        let b = image::open(&repeated)?.to_rgba8();
        anyhow::ensure!(a == b, "fixed-tick captures differ");
        std::fs::remove_file(repeated)?;
        cpu.sort_by(f64::total_cmp);
        states.insert(name.into(),serde_json::json!({"passes":timing_report(&samples),"visibility":renderer.visibility_stats(&mut gpu)?,"cpu_frame_record_submit_p50_ms":cpu[cpu.len()/2],"cpu_frame_record_submit_p95_ms":cpu[cpu.len()*95/100]}));
    }
    drop(rx);
    loader
        .join()
        .map_err(|_| anyhow::anyhow!("tile loader panicked"))??;
    let report = serde_json::json!({"device":gpu.info(),"resolution":[2560,1440],"terrain_m":[4096,4096],"instances":renderer.instance_count(),"height_content":content,"instance_content":instance_content,"tick":120,"seed":42,"warmup_frames":8,"measured_frames":32,"frames_in_flight":1,"detail_page_capacity":renderer.detail_page_capacity(),"tiles_streamed":streamed,"max_tile_upload_bytes_per_frame":13084,"capture_mode":"serialized diagnostics with asynchronous cache reads and bounded page publication","states":states});
    ara_gpu::cooked::write_json(&root.join("measurements/gpu-timings.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    gpu.assert_clean()?;
    renderer.retire(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
