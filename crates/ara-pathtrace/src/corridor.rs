//! Local energy diagnostics use the same camera, exposure and immutable reference.
use super::*;
use ara_pathtrace::metrics;

/// Extract the lower arcade region, excluding the sunlit upper courtyard.
pub(crate) fn roi(data: &[[f32; 4]]) -> Vec<[f32; 4]> {
    (235..350)
        .flat_map(|y| (170..430).map(move |x| data[y * 640 + x]))
        .collect()
}

pub(crate) fn run(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let scene = content::load(&root.join(".sn/assets/sponza"))?;
    let camera = content::camera();
    let ground = large::load_hdr(&root.join(".sn/sponza-reference.f32"))?;
    let mut results = Vec::new();
    let mut direct_reference =
        Reference::new(gpu, &scene, [640, 360], 2048, 1, large::fixed_post())?;
    direct_reference.converge(gpu, &camera)?;
    let direct_ground = direct_reference.radiance(gpu)?;
    direct_reference.destroy(gpu)?;
    for (name, events, frames) in [("material-integration", 1, 192)] {
        let mut s = settings([640, 360], [640, 360]);
        s.post = large::fixed_post();
        let mut view = FidelityView::new(gpu, &scene, s, None)?;
        view.set_gather_events(events)?;
        view.set_transport_stride(4)?;
        for tick in 0..frames {
            let mut f = input(&camera, tick, false);
            f.deterministic = false;
            view.render(gpu, &f)?;
        }
        let data = view.capture_radiance(gpu)?;
        let direct = view.capture_direct_radiance(gpu)?;
        large::save_hdr(&root.join(".sn/sponza-direct-hybrid.f32"), &direct)?;
        large::save_hdr(
            &root.join(".sn/sponza-direct-reference.f32"),
            &direct_ground,
        )?;
        println!(
            "Direct ROI: {:.5}",
            metrics::radiance(&roi(&direct), &roi(&direct_ground))?.relative_rmse
        );
        let regional = metrics::radiance(&roi(&data), &roi(&ground))?;
        let global = metrics::radiance(&data, &ground)?;
        println!(
            "{name}: ROI {:.5}, global {:.5}",
            regional.relative_rmse, global.relative_rmse
        );
        large::save_hdr(&root.join(format!(".sn/corridor-{name}.f32")), &data)?;
        view.capture(gpu)?
            .save(root.join(format!(".sn/corridor-{name}.png")))?;
        results.push(serde_json::json!({"name":name,"events":events,"frames":frames,"roi":regional,"global":global}));
        view.destroy(gpu)?;
    }
    ara_gpu::cooked::write_json(&root.join(".sn/corridor-diagnosis.json"), &results)?;
    gpu.assert_clean()?;
    Ok(())
}
