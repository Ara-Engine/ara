use anyhow::{Result, ensure};
use ara_gpu::{cooked::write_json, vulkan::VulkanDevice};
use ara_render::scene::CameraSpec;
use ara_render_geo::{DebugView, GeometryView, View};
use ara_texture_stream::Residency;
use glam::Vec3;
use std::{collections::BTreeMap, path::Path};

/// Diagnostic readbacks are excluded from steady-frame GPU benchmark totals.
pub fn measure(
    gpu: &mut VulkanDevice,
    view: &mut GeometryView,
    cache: &mut Residency,
    output: &Path,
    hardware_only: bool,
) -> Result<()> {
    let mut previous = BTreeMap::<(u32, u32, i32, i32), f32>::new();
    let mut compared = 0u64;
    let mut max_delta = 0f32;
    let mut max_lag = 0f32;
    let mut late = 0u64;
    let mut samples = 0u64;
    for frame in 0..61 {
        let shift = Vec3::new(frame as f32 * 3. / 60., 0., 0.);
        let camera = CameraSpec::LookAt {
            position: Vec3::new(18., 8., 22.) + shift,
            target: shift,
            vertical_fov: 55f32.to_radians(),
        };
        cache.prepare(gpu)?;
        let list = cache.begin_frame(gpu)?;
        let point = gpu.submit(list, &[])?;
        gpu.resolve(point)?;
        view.bind_textures(Some(cache.binding()))?;
        view.update(
            gpu,
            &View {
                camera: &camera,
                error_pixels: 1.,
                reset_history: frame == 0,
                debug: DebugView::Lit,
                hardware_only,
                full_resolution: false,
            },
        )?;
        let list = view.record(gpu)?;
        let point = gpu.submit(list, &[])?;
        view.submitted(point);
        gpu.resolve(point)?;
        let list = cache.end_frame(gpu)?;
        let point = gpu.submit(list, &[])?;
        cache.submitted(point);
        gpu.resolve(point)?;
        let stats = cache.stats(gpu)?;
        max_lag = max_lag.max(stats.max_mip_lag);
        late += u64::from(stats.samples_over_one_mip);
        samples += u64::from(stats.samples);
        let ids = view.identities(gpu)?;
        let surface = cache.capture_samples(gpu)?;
        let mut current = BTreeMap::<(u32, u32, i32, i32), f32>::new();
        for (id, sample) in ids.iter().zip(surface).step_by(8) {
            if id[0] == 0 {
                continue;
            }
            let key = (
                id[0],
                id[1],
                (sample[0] * 256.).floor() as i32,
                (sample[1] * 256.).floor() as i32,
            );
            current
                .entry(key)
                .and_modify(|m| *m = m.min(sample[3]))
                .or_insert(sample[3]);
        }
        for (key, mip) in &current {
            if let Some(old) = previous.get(key) {
                max_delta = max_delta.max((mip - old).abs());
                compared += 1;
            }
        }
        previous = current;
        if frame == 0 || frame == 60 {
            view.capture(gpu, &output.join(format!("motion-{frame}.png")))?;
        }
    }
    write_json(
        &output.join("texture-motion.json"),
        &serde_json::json!({"camera_speed_m_per_s":3,"fps":60,"frames":61,"source":"CC0 Coast Rocks 01, 8k albedo","stable_triangle_uv_comparisons":compared,"uv_bin_size":1./256.,"pixel_stride":8,"max_temporal_mip_delta":max_delta,"max_desired_resident_lag":max_lag,"samples_over_one_mip":late,"samples":samples,"texture_memory":cache.stats(gpu)?}),
    )?;
    ensure!(
        compared > 10000 && max_delta <= 1. && late == 0,
        "scan texture transition/lag bound failed: delta {max_delta}, lag {max_lag}, late {late}"
    );
    Ok(())
}
