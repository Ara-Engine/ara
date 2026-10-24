//! Same-view, fixed-exposure Sponza radiance comparisons, independent of post.
use super::*;
use ara_gpu::cooked::write_json;
use ara_pathtrace::metrics;

const SIZE: [u32; 2] = [640, 360];

fn scene(root: &Path) -> Result<ara_render::scene::Scene> {
    content::load(&root.join(".sn/assets/sponza"))
}
/// Fixed photographic display shared by hybrid/reference comparisons.
pub(crate) fn fixed_post() -> PostSettings {
    PostSettings {
        ev100: Some(5.5),
        tone_mapper: ara_render_fidelity::ToneMapper::PbrNeutral,
        bloom: 0.,
        fog_density: 0.,
        ..PostSettings::default()
    }
}
/// Store little-endian RGBA32F transport independently of the display transform.
pub(crate) fn save_hdr(path: &Path, data: &[[f32; 4]]) -> Result<()> {
    std::fs::write(
        path,
        data.iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    )?;
    Ok(())
}
/// Read the expected Sponza resolution from its ignored linear-radiance cache.
pub(crate) fn load_hdr(path: &Path) -> Result<Vec<[f32; 4]>> {
    let bytes = std::fs::read(path)?;
    anyhow::ensure!(
        bytes.len() == SIZE[0] as usize * SIZE[1] as usize * 16,
        "Reference dimensions differ"
    );
    Ok(bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|p| {
            std::array::from_fn(|i| {
                f32::from_le_bytes([p[i * 4], p[i * 4 + 1], p[i * 4 + 2], p[i * 4 + 3]])
            })
        })
        .collect())
}

/// Accumulate 32768 samples and check regional/full-frame residual against 16384.
pub fn reference(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let scene = scene(root)?;
    let camera = content::camera();
    let mut s = settings(SIZE, SIZE);
    s.post = fixed_post();
    let mut reference = Reference::new(gpu, &scene, SIZE, 32768, 12, s.post)?;
    let mut half = Vec::new();
    for batch in 0..4096 {
        reference.step(gpu, &camera, false)?;
        if batch == 2047 {
            half = reference.radiance(gpu)?;
        }
        if batch % 64 == 63 {
            println!("Sponza reference: {} / 32768 spp", (batch + 1) * 8);
        }
    }
    let ground = reference.radiance(gpu)?;
    save_hdr(&root.join(".sn/sponza-reference.f32"), &ground)?;
    reference
        .capture(gpu)?
        .save(output_path(root, "captures/sponza-reference.png")?)?;
    let convergence = metrics::radiance(&half, &ground)?;
    let regional = metrics::radiance(&corridor::roi(&half), &corridor::roi(&ground))?;
    println!("Reference corridor residual {:.5}", regional.relative_rmse);
    println!("Sponza reference residual {:.5}", convergence.relative_rmse);
    anyhow::ensure!(
        convergence.relative_rmse < 0.03 && regional.relative_rmse < 0.03,
        "Reference needs more samples"
    );
    write_json(
        &output_path(root, "measurements/sponza-reference.json")?,
        &serde_json::json!({"size":SIZE,"camera":camera,"scene_hash":reference.scene_hash(),"reference_samples":32768,"bounces":12,"ev100":5.5,"tone_mapper":"PBR Neutral","shader_revision":reference.shader_revision(),"precision":"RGBA32F linear accumulation, unfiltered","convergence":convergence,"corridor_convergence":regional}),
    )?;
    reference.destroy(gpu)?;
    gpu.assert_clean()?;
    Ok(())
}

/// Compare a current hybrid extraction with the cached, unfiltered ground truth.
pub fn quality(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let scene = scene(root)?;
    let camera = content::camera();
    let mut s = settings(SIZE, SIZE);
    s.post = fixed_post();
    let mut view = FidelityView::new(gpu, &scene, s, None)?;
    view.set_gather_events(1)?;
    view.set_transport_stride(4)?;
    for tick in 0..192 {
        let mut request = input(&camera, tick, false);
        request.deterministic = false;
        view.render(gpu, &request)?;
    }
    let data = view.capture_radiance(gpu)?;
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(output_path(
        root,
        "measurements/sponza-reference.json",
    )?)?)?;
    anyhow::ensure!(
        manifest["scene_hash"].as_str() == Some(view.scene_hash())
            && manifest["shader_revision"].as_str() == Some(view.shader_revision()),
        "Cached reference scene or shader cohort differs"
    );
    let ground = load_hdr(&root.join(".sn/sponza-reference.f32"))?;
    display(&ground, SIZE, fixed_post())
        .save(output_path(root, "captures/sponza-reference.png")?)?;
    anyhow::ensure!(
        manifest["size"] == serde_json::json!(SIZE)
            && manifest["camera"] == serde_json::to_value(camera)?
            && manifest["reference_samples"]
                .as_u64()
                .is_some_and(|n| n >= 4096)
            && manifest["bounces"] == 12
            && manifest["convergence"]["relative_rmse"]
                .as_f64()
                .is_some_and(|r| (0.0..0.03).contains(&r)),
        "Reference camera, sample or convergence contract differs"
    );
    let error = metrics::radiance(&data, &ground)?;
    let regional = metrics::radiance(&corridor::roi(&data), &corridor::roi(&ground))?;
    anyhow::ensure!(
        error.relative_rmse <= 0.10,
        "Sponza exceeds 10% relative RMSE: {}",
        error.relative_rmse
    );
    println!("Sponza hybrid: relative RMSE {:.5}", error.relative_rmse);
    println!(
        "Sponza corridor: relative RMSE {:.5}",
        regional.relative_rmse
    );
    let hybrid = view.capture(gpu)?;
    hybrid.save(output_path(root, "captures/sponza-hybrid.png")?)?;
    let golden = root.join("tests/goldens/sponza.png");
    if std::env::var("ARA_UPDATE_FIDELITY_GOLDENS").as_deref() == Ok("1") {
        hybrid.save(&golden)?;
    }
    let comparison =
        ara_render::capture::compare(&hybrid, &image::open(golden)?.to_rgba8(), 0.005)?;
    anyhow::ensure!(
        comparison.passed,
        "Sponza display golden changed: {}",
        comparison.score
    );
    side_by_side(&hybrid, &display(&ground, SIZE, fixed_post())).save(output_path(
        root,
        "captures/sponza-reference-side-by-side.png",
    )?)?;
    save_hdr(&root.join(".sn/sponza-hybrid.f32"), &data)?;
    let bake = view.bake_snapshot(gpu)?;
    let active = bake
        .data
        .as_chunks::<102>()
        .0
        .iter()
        .filter(|p| p[100][3] > 0.5)
        .count();
    write_json(
        &output_path(root, "measurements/sponza-quality.json")?,
        &serde_json::json!({"size":SIZE,"settle_frames":192,"sampling":"256-phase Halton pixel-area integration including remodulation; four-phase transport; one-event lobe mixture; mip-filtered materials","ev100":5.5,"tone_mapper":"PBR Neutral","error":error,"corridor_roi":[170,235,430,350],"corridor_error":regional,"scene_hash":view.scene_hash(),"shader_revision":view.shader_revision(),"volume":bake.volume,"active_probes":active,"probes":bake.volume.len()}),
    )?;
    // Exercise real content with camera reprojection and four-phase repair.
    let mut sequence = image::RgbaImage::new(SIZE[0] * 4, SIZE[1] * 3);
    for tick in 0..48 {
        let pan = ara_render::scene::CameraSpec::LookAt {
            position: glam::Vec3::new(-11. + tick as f32 * 0.01, 1.7, tick as f32 * 0.003),
            target: glam::Vec3::new(5., 3., 0.),
            vertical_fov: 1.05,
        };
        let mut frame = input(&pan, 192 + tick, false);
        frame.deterministic = false;
        view.render(gpu, &frame)?;
        if tick % 4 == 3 {
            let i = tick / 4;
            image::imageops::replace(
                &mut sequence,
                &view.capture(gpu)?,
                i64::from(i % 4 * SIZE[0]),
                i64::from(i / 4 * SIZE[1]),
            );
        }
    }
    sequence.save(output_path(root, "captures/sponza-pan-sequence.png")?)?;
    view.destroy(gpu)?;
    gpu.assert_clean()?;
    anyhow::ensure!(regional.relative_rmse <= 0.12, "Corridor exceeds 12% RMSE");
    Ok(())
}

/// Identical deterministic photographic display for saved unfiltered reference radiance.
pub(crate) fn display(data: &[[f32; 4]], size: [u32; 2], post: PostSettings) -> image::RgbaImage {
    let exposure = 2_f32.powf(post.compensation)
        / (1.2 * 2_f32.powf(post.ev100.expect("Offline captures require fixed exposure")));
    image::RgbaImage::from_fn(size[0], size[1], |x, y| {
        let p = data[(y * size[0] + x) as usize];
        let c = ara_render_fidelity::colour::tone_map(
            glam::Vec3::new(p[0], p[1], p[2]) * exposure,
            post.tone_mapper,
        );
        let rgb = c.to_array().map(|v| {
            if v <= 0.0031308 {
                12.92 * v
            } else {
                1.055 * v.powf(1. / 2.4) - 0.055
            }
        });
        image::Rgba([
            rgb[0].clamp(0., 1.).mul_add(255., 0.5) as u8,
            rgb[1].clamp(0., 1.).mul_add(255., 0.5) as u8,
            rgb[2].clamp(0., 1.).mul_add(255., 0.5) as u8,
            255,
        ])
    })
}
/// Place matching-resolution comparison readbacks side by side.
pub(crate) fn side_by_side(a: &image::RgbaImage, b: &image::RgbaImage) -> image::RgbaImage {
    let mut image = image::RgbaImage::new(a.width() * 2, a.height());
    image::imageops::replace(&mut image, a, 0, 0);
    image::imageops::replace(&mut image, b, i64::from(a.width()), 0);
    image
}
