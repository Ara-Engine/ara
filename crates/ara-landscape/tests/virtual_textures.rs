use anyhow::{Result, ensure};
use ara_gpu::{cooked::write_json, vulkan::VulkanDevice};
use ara_landscape::{Landscape, View, sample};
use ara_texture_stream::{Catalog, Residency, Source};
use std::path::Path;

#[test]
fn triplanar_materials_share_gpu_texture_feedback_and_budget() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("captures/virtual-textures");
    std::fs::create_dir_all(&output)?;
    let mut data = sample::landscape(None)?;
    data.instances.truncate(512);
    let mut gpu = VulkanDevice::new(&[], root.join("cache/virtual-textures"))?;
    let size = [640, 360];
    let mut landscape = Landscape::new(&mut gpu, &data, size, &root.join("cooked"))?;
    let mut before = Vec::new();
    let mut after = Vec::new();
    for frame in 0..16 {
        landscape.update(&gpu, &View::noon())?;
        let list = landscape.record(&gpu)?;
        let p = gpu.submit(list, &[])?;
        let times = gpu.resolve(p)?;
        if frame >= 8 {
            before.push(times);
        }
    }
    landscape.capture(&mut gpu, &output.join("classic.png"))?;
    let pixels: Vec<_> = (0..2048 * 2048u32)
        .flat_map(|p| {
            let c = if (((p % 2048) / 64) ^ ((p / 2048) / 64)) & 1 == 0 {
                100
            } else {
                180
            };
            [c, 180, 80, 255]
        })
        .collect();
    let catalog = Catalog::new(vec![Source::rgba8([2048, 2048], pixels, true, 50)?])?;
    let mut textures = Residency::new(
        &mut gpu,
        catalog,
        16 * 1024 * 1024,
        32,
        size,
        &root.join("../ara-texture-stream/cooked"),
    )?;
    for frame in 0..40 {
        textures.prepare(&gpu)?;
        let list = textures.begin_frame(&gpu)?;
        let p = gpu.submit(list, &[])?;
        gpu.resolve(p)?;
        landscape.bind_virtual_textures(Some((textures.binding(), [0; 4])))?;
        landscape.update(&gpu, &View::noon())?;
        let list = landscape.record(&gpu)?;
        let p = gpu.submit(list, &[])?;
        let times = gpu.resolve(p)?;
        let list = textures.end_frame(&gpu)?;
        let p = gpu.submit(list, &[])?;
        textures.submitted(p);
        gpu.resolve(p)?;
        if frame >= 24 {
            after.push(times);
        }
    }
    let stats = textures.stats(&gpu)?;
    ensure!(
        stats.samples > 10000
            && stats.pool_bytes <= 16 * 1024 * 1024
            && stats.samples_over_one_mip == 0,
        "landscape virtual sampling failed: {}",
        serde_json::to_string(&stats)?
    );
    landscape.capture(&mut gpu, &output.join("streamed.png"))?;
    write_json(
        &output.join("timings.json"),
        &serde_json::json!({"classic":ara_landscape::timing_report(&before),"streamed":ara_landscape::timing_report(&after),"texture_memory":stats,"background_load":"Shared workstation; two build jobs"}),
    )?;
    gpu.assert_clean()?;
    landscape.retire(&mut gpu)?;
    textures.retire(&mut gpu)?;
    Ok(())
}
