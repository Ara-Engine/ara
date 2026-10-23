//! Focused low-light diagnostics and the real-content/many-light verification views.
use super::*;
use ara_gpu::cooked::write_json;
use ara_pathtrace::metrics;
const SIZE: [u32; 2] = [480, 270];

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
    hybrid.save(output_path(root, format!("captures/{name}-hybrid.png"))?)?;
    view.destroy(gpu)?;
    let samples = if many { 65536 } else { 4096 };
    let batches = samples / 8;
    let mut reference = Reference::new(gpu, &scene, size, samples, 12, s.post)?;
    if !many {
        reference.set_atmosphere(Some(Default::default()))?;
    }
    let mut half = Vec::new();
    for batch in 0..batches {
        reference.step(gpu, &camera, false)?;
        if batch + 1 == batches / 2 {
            half = reference.radiance(gpu)?;
        }
        if batch % 64 == 63 {
            println!("{name}: {} /{samples} spp", (batch + 1) * 8);
        }
    }
    let ground = reference.radiance(gpu)?;
    let picture = reference.capture(gpu)?;
    picture.save(output_path(root, format!("captures/{name}-reference.png"))?)?;
    large::side_by_side(&hybrid, &picture).save(output_path(
        root,
        format!("captures/{name}-side-by-side.png"),
    )?)?;
    let error = metrics::radiance(&radiance, &ground)?;
    let convergence = metrics::radiance(&half, &ground)?;
    let lower = (size[0] * size[1] / 2) as usize;
    let surface_roi_error = metrics::radiance(&radiance[lower..], &ground[lower..])?;
    let surface_roi_convergence = metrics::radiance(&half[lower..], &ground[lower..])?;
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
        &output_path(root, format!("measurements/{name}-quality.json"))?,
        &serde_json::json!({"size":size,"reference_samples":samples,"bounces":12,"ev100":ev,"scene_hash":hash,"shader_revision":shader,"error":error,"convergence":convergence,"surface_roi":"lower half","surface_roi_error":surface_roi_error,"surface_roi_convergence":surface_roi_convergence,"point_lights":scene.lights.len(),"triangles":scene.meshes.iter().map(|m|m.indices.len()/3).sum::<usize>(),"source":if many{"500 points + 500 emissive panels, 1000 semantic lights"}else{"CC0 Poly Haven PBR furniture; see THIRD_PARTY_NOTICES.md"},"reference_model":if many {"independent surface path transport"} else {"independent surface transport; shared sky/froxel composition, HDR error before fog"}}),
    )?;
    large::save_hdr(&root.join(format!(".sn/{name}-reference.f32")), &ground)?;
    reference.destroy(gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
