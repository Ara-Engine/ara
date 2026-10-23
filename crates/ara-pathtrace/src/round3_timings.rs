//! Warm-cache beauty/many-light per-pass GPU distributions with background telemetry.
use super::*;
use ara_gpu::cooked::write_json;
use std::collections::BTreeMap;

pub fn run(gpu: &mut VulkanDevice, root: &Path, many: bool) -> Result<()> {
    let (name, scene, camera, ev) = round3::case(root, many)?;
    let mut report = Vec::new();
    for render in [[2560, 1440], [1707, 960]] {
        let mut s = settings(render, [2560, 1440]);
        s.post = PostSettings {
            ev100: None,
            compensation: -1.,
            ..large::fixed_post()
        };
        let mut view = FidelityView::new(gpu, &scene, s, None)?;
        round3::configure(gpu, &mut view, many)?;
        let before = content::background_load();
        let cpu_before = content::cpu_load();
        let mut totals = Vec::new();
        let mut passes = BTreeMap::<String, Vec<f64>>::new();
        for tick in 0..336 {
            let mut f = input(&camera, tick, false);
            f.deterministic = false;
            view.render(gpu, &f)?;
            if tick >= 96 {
                totals.push(view.timings().iter().map(|p| p.milliseconds).sum::<f64>());
                for p in view.timings() {
                    passes
                        .entry(p.pass.to_owned())
                        .or_default()
                        .push(p.milliseconds);
                }
            }
        }
        let frame_samples = totals.to_vec();
        totals.sort_by(f64::total_cmp);
        let passes = passes
            .into_iter()
            .map(|(k, mut v)| {
                v.sort_by(f64::total_cmp);
                (k, serde_json::json!({"p50":v[120],"p95":v[227]}))
            })
            .collect::<BTreeMap<_, _>>();
        let exposure = view.capture_exposure(gpu)?;
        println!(
            "{name} {}p: p50 {:.3}, p95 {:.3} ms",
            render[1], totals[120], totals[227]
        );
        report.push(serde_json::json!({"scene":name,"render_size":render,"display_size":[2560,1440],"frames":240,"warmup":96,"p50":totals[120],"p95":totals[227],"maximum_ms":totals.last(),"frame_samples_ms":frame_samples,"passes":passes,"scene_hash":view.scene_hash(),"shader_revision":view.shader_revision(),"point_lights":scene.lights.len(),"background_before":before,"background_after":content::background_load(),"cpu_before":cpu_before,"cpu_after":content::cpu_load(),"exposure":exposure,"shared_gpu":true,"async_overlap":false}));
        view.set_post(PostSettings {
            ev100: Some(ev),
            ..large::fixed_post()
        })?;
        view.render(gpu, &input(&camera, 336, false))?;
        view.capture(gpu)?.save(output_path(
            root,
            format!("captures/{name}-{}p.png", render[1]),
        )?)?;
        view.destroy(gpu)?;
    }
    write_json(
        &output_path(root, format!("measurements/{name}-timings.json"))?,
        &report,
    )?;
    gpu.assert_clean()?;
    Ok(())
}
