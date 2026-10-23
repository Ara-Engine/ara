//! Reproducible daylight landscape and emissive-only night quality/performance runs.
use super::*;
use ara_gpu::cooked::write_json;
use ara_pathtrace::{metrics, robust_scenes};
use std::collections::BTreeMap;

const SIZE: [u32; 2] = [480, 270];
fn configure(gpu: &mut VulkanDevice, view: &mut FidelityView, name: &str) -> Result<()> {
    robust_scenes::configure_view(gpu, view, name == "outdoor_day")
}

fn cases() -> Result<
    Vec<(
        &'static str,
        ara_render::scene::Scene,
        ara_render::scene::CameraSpec,
        f32,
    )>,
> {
    Ok(vec![
        (
            "outdoor_day",
            robust_scenes::outdoor()?,
            robust_scenes::camera(true),
            12.,
        ),
        (
            "night_interior",
            robust_scenes::night(),
            robust_scenes::camera(false),
            5.,
        ),
    ])
}

/// Record independently converged references and compare deterministic display goldens.
pub fn quality(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let mut report = Vec::new();
    let update = std::env::var("ARA_UPDATE_FIDELITY_GOLDENS").as_deref() == Ok("1");
    for (name, scene, camera, ev) in cases()? {
        let post = PostSettings {
            ev100: Some(ev),
            ..large::fixed_post()
        };
        let mut s = settings(SIZE, SIZE);
        s.post = post;
        let mut view = FidelityView::new(gpu, &scene, s, None)?;
        configure(gpu, &mut view, name)?;
        for tick in 0..256 {
            let mut frame = input(&camera, tick, false);
            frame.deterministic = false;
            view.render(gpu, &frame)?;
        }
        let hybrid = view.capture(gpu)?;
        let hdr = view.capture_radiance(gpu)?;
        hybrid.save(root.join(format!("captures/{name}-hybrid.png")))?;
        let hash = view.scene_hash().to_owned();
        view.destroy(gpu)?;
        let mut reference = Reference::new(gpu, &scene, SIZE, 4096, 12, post)?;
        if name == "outdoor_day" {
            reference.set_atmosphere(Some(Default::default()))?;
        }
        let mut half = Vec::new();
        for batch in 0..512 {
            reference.step(gpu, &camera, false)?;
            if batch == 255 {
                half = reference.radiance(gpu)?;
            }
            if batch % 64 == 63 {
                println!("{name} reference: {} / 4096 spp", (batch + 1) * 8);
            }
        }
        let ground = reference.radiance(gpu)?;
        let reference_image = reference.capture(gpu)?;
        reference_image.save(root.join(format!("captures/{name}-reference.png")))?;
        large::side_by_side(&hybrid, &reference_image)
            .save(root.join(format!("captures/{name}-side-by-side.png")))?;
        let error = metrics::radiance(&hdr, &ground)?;
        let convergence = metrics::radiance(&half, &ground)?;
        let golden = root.join(format!("tests/goldens/{name}.png"));
        if update {
            hybrid.save(&golden)?;
        }
        {
            let result =
                ara_render::capture::compare(&hybrid, &image::open(&golden)?.to_rgba8(), 0.005)?;
            anyhow::ensure!(result.passed, "{name}: golden error {}", result.score);
        }
        println!(
            "{name}: relative RMSE {:.5}; convergence {:.5}",
            error.relative_rmse, convergence.relative_rmse
        );
        report.push(serde_json::json!({"scene":name,"scene_hash":hash,"size":SIZE,"camera":camera,"ev100":ev,"tone_mapper":"PBR Neutral","samples":4096,"bounces":12,"settle_frames":256,"error":error,"convergence":convergence}));
        reference.destroy(gpu)?;
    }
    write_json(&root.join("measurements/robust-quality.json"), &report)?;
    gpu.assert_clean()?;
    Ok(())
}

/// Measure complete native/upscaled frames; new fixtures have no historical baseline.
pub fn timings(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let mut report = Vec::new();
    let mut budgets_passed = true;
    for (name, scene, camera, ev) in cases()? {
        for render in [[2560, 1440], [1707, 960]] {
            let mut s = settings(render, [2560, 1440]);
            s.post = PostSettings {
                ev100: None,
                compensation: -1.,
                ..large::fixed_post()
            };
            let mut view = FidelityView::new(gpu, &scene, s, None)?;
            configure(gpu, &mut view, name)?;
            let mut totals = Vec::new();
            let mut passes = BTreeMap::<String, Vec<f64>>::new();
            for tick in 0..156 {
                let mut frame = input(&camera, tick, false);
                frame.deterministic = false;
                view.render(gpu, &frame)?;
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
            let exposure = view.capture_exposure(gpu)?;
            // Hero images use a fixed photographic exposure; measurement includes auto metering.
            view.set_post(PostSettings {
                ev100: Some(ev),
                ..large::fixed_post()
            })?;
            view.render(gpu, &input(&camera, 156, false))?;
            view.capture(gpu)?
                .save(root.join(format!("captures/{name}-{}p.png", render[1])))?;
            totals.sort_by(f64::total_cmp);
            budgets_passed &= totals[56] <= if render[1] == 1440 { 10. } else { 5. };
            let passes = passes
                .into_iter()
                .map(|(name, mut times)| {
                    times.sort_by(f64::total_cmp);
                    (name, serde_json::json!({"p50":times[30],"p95":times[56]}))
                })
                .collect::<BTreeMap<_, _>>();
            println!(
                "{name} {}p: p50 {:.3}, p95 {:.3} ms",
                render[1], totals[30], totals[56]
            );
            report.push(serde_json::json!({"scene":name,"render_size":render,"display_size":[2560,1440],"p50":totals[30],"p95":totals[56],"budget_ms":if render[1]==1440 {10.} else {5.},"budget_passed":totals[56]<=if render[1]==1440 {10.} else {5.},"warmup":96,"frames":60,"passes":passes,"exposure":exposure,"scene_hash":view.scene_hash(),"triangles":scene.meshes.iter().map(|m|m.indices.len()/3).sum::<usize>(),"background_load":"shared GPU with approximately ten other agents; no forced clocks or exclusive access","async_overlap":false}));
            view.destroy(gpu)?;
        }
    }
    write_json(&root.join("measurements/robust-timings.json"), &report)?;
    write_json(
        &root.join("measurements/validation.json"),
        &gpu.diagnostics(),
    )?;
    gpu.assert_clean()?;
    anyhow::ensure!(
        budgets_passed,
        "Robust scene p95 budget failed; see robust-timings.json"
    );
    Ok(())
}
