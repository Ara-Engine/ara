mod support;

use anyhow::{Result, ensure};
use ara_geo::{SourceMesh, Vertex};
use ara_gpu::{GpuDevice, cooked::write_json};
use ara_render::scene::CameraSpec;
use ara_render_geo::{DebugView, GeometryView, Instance, View};
use ara_texture_stream::{Catalog, Residency, Source};
use glam::Vec3;
use std::{collections::BTreeMap, path::Path};

#[test]
fn moving_camera_measures_resident_mip_lag_and_same_uv_transitions() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("captures/texture-motion");
    std::fs::create_dir_all(&output)?;
    let mut gpu = GpuDevice::new(&[], output.join("cache"))?;
    // Atomic visibility needs 64-bit buffer atomics (Metal has none yet): skip with the
    // reason `GeometryView` gives, like the product path.
    if !gpu.info().buffer_int64_atomics {
        eprintln!(
            "skipped: ARA_GEO_CAPABILITY: shaderBufferInt64Atomics required for atomic visibility"
        );
        return Ok(());
    }
    let source = SourceMesh {
        vertices: [
            [-16., 0., -16.],
            [-16., 0., 16.],
            [16., 0., 16.],
            [16., 0., -16.],
        ]
        .into_iter()
        .map(|position| Vertex {
            position,
            normal: [0., 1., 0.],
            uv: [position[0] / 8., position[2] / 8.],
        })
        .collect(),
        indices: vec![0, 1, 2, 0, 2, 3],
    };
    let geometry = ara_geo::cook(&source, Default::default())?;
    let instance = Instance {
        position_scale: [0., 0., 0., 1.],
        color_roughness: [1., 1., 1., 0.8],
        identity: [1, 0, 0, 0],
    };
    let size = [320, 180];
    let mut view = GeometryView::new(
        &mut gpu,
        &geometry,
        &[instance],
        size,
        64,
        &root.join("cooked"),
        true,
    )?;
    let pixels: Vec<_> = (0..2048 * 2048u32)
        .flat_map(|p| {
            let x = p % 2048;
            let y = p / 2048;
            let c = if ((x / 16) ^ (y / 16)) & 1 == 0 {
                220
            } else {
                40
            };
            [c, (x / 8) as u8, (y / 8) as u8, 255]
        })
        .collect();
    let mut cache = Residency::new(
        &mut gpu,
        Catalog::new(vec![Source::rgba8([2048, 2048], pixels, true, 10)?])?,
        16 * 1024 * 1024,
        32,
        size,
        &root.join("../ara-texture-stream/cooked"),
    )?;
    let mut previous = BTreeMap::<(i32, i32), f32>::new();
    let mut max_delta = 0f32;
    let mut max_lag = 0f32;
    let mut compared = 0u64;
    let mut over_one = 0u64;
    let mut sampled = 0u64;
    for frame in 0..90 {
        let x = if frame < 30 {
            0.
        } else {
            (frame - 30) as f32 / 60.
        };
        let camera = CameraSpec::LookAt {
            position: Vec3::new(x, 3., 5.),
            target: Vec3::new(x, 0., 0.),
            vertical_fov: 55f32.to_radians(),
        };
        cache.prepare(&gpu)?;
        let list = cache.begin_frame(&gpu)?;
        let p = gpu.submit(list, &[])?;
        gpu.resolve(p)?;
        view.bind_textures(Some(cache.binding()))?;
        view.update(
            &mut gpu,
            &View {
                camera: &camera,
                error_pixels: 1.,
                reset_history: frame == 0,
                debug: DebugView::Lit,
                hardware_only: false,
                full_resolution: false,
            },
        )?;
        let list = view.record(&gpu)?;
        let p = gpu.submit(list, &[])?;
        view.submitted(p);
        gpu.resolve(p)?;
        let list = cache.end_frame(&gpu)?;
        let p = gpu.submit(list, &[])?;
        cache.submitted(p);
        gpu.resolve(p)?;
        if frame >= 30 {
            let stats = cache.stats(&gpu)?;
            max_lag = max_lag.max(stats.max_mip_lag);
            over_one += u64::from(stats.samples_over_one_mip);
            sampled += u64::from(stats.samples);
            let ids = view.identities(&mut gpu)?;
            let samples = cache.capture_samples(&mut gpu)?;
            let mut current = BTreeMap::<(i32, i32), f32>::new();
            for (id, sample) in ids.iter().zip(samples) {
                if id[0] != 0 {
                    let key = (
                        (sample[0] * 256.).floor() as i32,
                        (sample[1] * 256.).floor() as i32,
                    );
                    current
                        .entry(key)
                        .and_modify(|m| *m = m.min(sample[3]))
                        .or_insert(sample[3]);
                }
            }
            for (key, mip) in &current {
                if let Some(old) = previous.get(key) {
                    max_delta = max_delta.max((mip - old).abs());
                    compared += 1;
                }
            }
            previous = current;
        }
        if frame == 30 || frame == 89 {
            view.capture(&mut gpu, &output.join(format!("frame-{frame}.png")))?;
        }
    }
    write_json(
        &output.join("metrics.json"),
        &serde_json::json!({"camera_speed_m_per_s":1,"fps":60,"warmup_frames":30,"measured_frames":60,"uv_bin_size":1./256.,"same_uv_comparisons":compared,"max_temporal_mip_delta":max_delta,"max_desired_resident_lag":max_lag,"samples_over_one_mip":over_one,"samples":sampled,"texture_memory":cache.stats(&gpu)?}),
    )?;
    ensure!(
        compared > 10000 && max_delta <= 1. && over_one == 0,
        "texture transition/lag bound failed: delta {max_delta}, lag {max_lag}, samples over one {over_one}"
    );
    let pixels = view.preview_bytes(&mut gpu)?;
    let golden = root.join("goldens/textured.png");
    if std::env::var_os("ARA_UPDATE_GEO_GOLDENS").is_some() {
        image::save_buffer(&golden, &pixels, size[0], size[1], image::ColorType::Rgba8)?;
    }
    ensure!(
        support::matches_golden(&pixels, &image::open(&golden)?.to_rgba8().into_raw()),
        "textured golden differs; inspect before deliberate update"
    );
    gpu.assert_clean()?;
    view.retire(&mut gpu)?;
    cache.retire(&mut gpu)?;
    Ok(())
}
