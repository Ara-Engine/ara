//! Focused low-light diagnostics and the real-content/many-light verification views.
use super::*;
use ara_gpu::cooked::write_json;
use ara_pathtrace::metrics;
const SIZE: [u32; 2] = [480, 270];

fn hdr(path: &Path) -> Result<Vec<[f32; 4]>> {
    let bytes = std::fs::read(path)?;
    anyhow::ensure!(
        bytes.len() == (SIZE[0] * SIZE[1] * 16) as usize,
        "HDR size differs"
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
fn save_hdr(path: &Path, data: &[[f32; 4]]) -> Result<()> {
    std::fs::write(
        path,
        data.iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<_>>(),
    )?;
    Ok(())
}
pub(crate) fn configure(
    gpu: &mut VulkanDevice,
    view: &mut FidelityView,
    reservoirs: bool,
) -> Result<()> {
    view.set_gather_events(if reservoirs { 2 } else { 1 })?;
    view.set_transport_stride(if reservoirs { 2 } else { 4 })?;
    view.set_reservoir_sampling(gpu, reservoirs)?;
    if !reservoirs {
        view.set_atmosphere(Some(Default::default()))?;
    }
    Ok(())
}
/// Short cached-reference probe; results are explicitly prototype diagnostics.
pub fn night_probe(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let scene = ara_pathtrace::robust_scenes::night();
    let camera = ara_pathtrace::robust_scenes::camera(false);
    let mut settings = settings(SIZE, SIZE);
    settings.post = PostSettings {
        ev100: Some(5.),
        ..large::fixed_post()
    };
    let mut view = FidelityView::new(gpu, &scene, settings, None)?;
    configure(gpu, &mut view, true)?;
    for tick in 0..256 {
        let mut frame = input(&camera, tick, false);
        frame.deterministic = false;
        view.render(gpu, &frame)?;
    }
    let hybrid = view.capture_radiance(gpu)?;
    let reference = hdr(&root.join(".sn/night_interior-reference.f32"))?;
    let lower = (SIZE[0] * SIZE[1] / 2) as usize;
    let error = metrics::radiance(&hybrid[lower..], &reference[lower..])?;
    println!("Night prototype floor ROI: {:.5}", error.relative_rmse);
    view.capture(gpu)?
        .save(root.join(".sn/night-reservoir-prototype.png"))?;
    save_hdr(&root.join(".sn/night-reservoir-prototype.f32"), &hybrid)?;
    let direct = view.capture_direct_radiance(gpu)?;
    save_hdr(&root.join(".sn/night-reservoir-direct.f32"), &direct)?;
    write_json(
        &root.join(".sn/night-reservoir-prototype.json"),
        &serde_json::json!({"error":error,"reference":"prototype cache: final comparison must regenerate independent reference","shader_revision":view.shader_revision()}),
    )?;
    view.destroy(gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
/// Independently path trace and enforce the main beauty/stress display golden.
pub(crate) fn case(
    root: &Path,
    many: bool,
) -> Result<(
    &'static str,
    ara_render::scene::Scene,
    ara_render::scene::CameraSpec,
    f32,
)> {
    Ok(if many {
        (
            "many_lights",
            ara_pathtrace::many_lights::scene(),
            ara_pathtrace::many_lights::camera(),
            6.,
        )
    } else {
        (
            "beauty_cc0",
            ara_pathtrace::beauty::scene(&root.join(".sn/assets/beauty"))?,
            ara_pathtrace::beauty::camera(),
            8.,
        )
    })
}
pub fn quality(gpu: &mut VulkanDevice, root: &Path, many: bool) -> Result<()> {
    let (name, scene, camera, ev) = case(root, many)?;
    let size = if many { SIZE } else { [640, 360] };
    let mut s = settings(size, size);
    s.post = PostSettings {
        ev100: Some(ev),
        ..large::fixed_post()
    };
    let mut view = FidelityView::new(gpu, &scene, s, None)?;
    configure(gpu, &mut view, many)?;
    for tick in 0..256 {
        let mut f = input(&camera, tick, false);
        f.deterministic = false;
        view.render(gpu, &f)?;
    }
    let hybrid = view.capture(gpu)?;
    let radiance = view.capture_radiance(gpu)?;
    let hash = view.scene_hash().to_owned();
    let shader = view.shader_revision().to_owned();
    hybrid.save(root.join(format!("captures/{name}-hybrid.png")))?;
    view.destroy(gpu)?;
    let mut reference = Reference::new(gpu, &scene, size, 4096, 12, s.post)?;
    if !many {
        reference.set_atmosphere(Some(Default::default()))?;
    }
    let mut half = Vec::new();
    for batch in 0..512 {
        reference.step(gpu, &camera, false)?;
        if batch == 255 {
            half = reference.radiance(gpu)?;
        }
        if batch % 64 == 63 {
            println!("{name}: {} /4096 spp", (batch + 1) * 8);
        }
    }
    let ground = reference.radiance(gpu)?;
    let picture = reference.capture(gpu)?;
    picture.save(root.join(format!("captures/{name}-reference.png")))?;
    large::side_by_side(&hybrid, &picture)
        .save(root.join(format!("captures/{name}-side-by-side.png")))?;
    let error = metrics::radiance(&radiance, &ground)?;
    let convergence = metrics::radiance(&half, &ground)?;
    println!(
        "{name}: relative RMSE {:.5}, reference residual {:.5}",
        error.relative_rmse, convergence.relative_rmse
    );
    let golden = root.join(format!("tests/goldens/{name}.png"));
    if std::env::var("ARA_UPDATE_FIDELITY_GOLDENS").as_deref() == Ok("1") {
        hybrid.save(&golden)?;
    }
    let comparison =
        ara_render::capture::compare(&hybrid, &image::open(golden)?.to_rgba8(), 0.005)?;
    anyhow::ensure!(
        comparison.passed,
        "{name}: golden error {}",
        comparison.score
    );
    write_json(
        &root.join(format!("measurements/{name}-quality.json")),
        &serde_json::json!({"size":size,"reference_samples":4096,"bounces":12,"ev100":ev,"scene_hash":hash,"shader_revision":shader,"error":error,"convergence":convergence,"point_lights":scene.lights.len(),"triangles":scene.meshes.iter().map(|m|m.indices.len()/3).sum::<usize>(),"source":if many{"500 points + 500 emissive panels, 1000 semantic lights"}else{"CC0 Poly Haven PBR furniture; see THIRD_PARTY_NOTICES.md"}}),
    )?;
    save_hdr(&root.join(format!(".sn/{name}-reference.f32")), &ground)?;
    reference.destroy(gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
