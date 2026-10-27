//! Reproducible daylight landscape and emissive-only night quality/performance runs.
use super::*;
use ara_gpu::cooked::write_json;
use ara_pathtrace::{metrics, robust_scenes};
use std::collections::BTreeMap;

const SIZE: [u32; 2] = [480, 270];
fn configure(gpu: &mut GpuDevice, view: &mut FidelityView, name: &str) -> Result<()> {
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
pub fn quality(gpu: &mut GpuDevice, root: &Path) -> Result<()> {
    let mut report = Vec::new();
    let mut passed = true;
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
        hybrid.save(output_path(root, format!("captures/{name}-hybrid.png"))?)?;
        let hash = view.scene_hash().to_owned();
        let revision = view.shader_revision().to_owned();
        view.destroy(gpu)?;
        let samples = if name == "night_interior" {
            32768
        } else {
            4096
        };
        let batches = samples / 8;
        let mut reference = Reference::new(gpu, &scene, SIZE, samples, 12, post)?;
        if name == "outdoor_day" {
            reference.set_atmosphere(Some(Default::default()))?;
        }
        let mut half = Vec::new();
        for batch in 0..batches {
            reference.step(gpu, &camera, false)?;
            if batch + 1 == batches / 2 {
                half = reference.radiance(gpu)?;
            }
            if batch % 64 == 63 {
                println!("{name} reference: {} / {samples} spp", (batch + 1) * 8);
            }
        }
        let ground = reference.radiance(gpu)?;
        let reference_image = reference.capture(gpu)?;
        reference_image.save(output_path(root, format!("captures/{name}-reference.png"))?)?;
        large::side_by_side(&hybrid, &reference_image).save(output_path(
            root,
            format!("captures/{name}-side-by-side.png"),
        )?)?;
        let error = metrics::radiance(&hdr, &ground)?;
        let lower = (SIZE[0] * SIZE[1] / 2) as usize;
        let surface_roi_error = metrics::radiance(&hdr[lower..], &ground[lower..])?;
        passed &= if name == "night_interior" {
            surface_roi_error.relative_rmse <= 0.08
        } else {
            error.relative_rmse <= 0.1
        };
        large::save_hdr(&root.join(format!(".sn/{name}-hybrid.f32")), &hdr)?;
        large::save_hdr(&root.join(format!(".sn/{name}-reference.f32")), &ground)?;
        let convergence = metrics::radiance(&half, &ground)?;
        let surface_roi_convergence = metrics::radiance(&half[lower..], &ground[lower..])?;
        println!(
            "{name}: surface ROI {:.5}, reference ROI residual {:.5}",
            surface_roi_error.relative_rmse, surface_roi_convergence.relative_rmse
        );
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
        report.push(serde_json::json!({"scene":name,"scene_hash":hash,"shader_revision":revision,"reference_model":if name=="outdoor_day"{"independent path-traced surface transport; shared sky LUT and froxel display composition; HDR error measured before fog"}else{"independent surface path transport"},"surface_roi":if name=="outdoor_day"{"lower half: terrain and foliage"}else{"lower half: floor and sphere bases"},"surface_roi_error":surface_roi_error,"size":SIZE,"camera":camera,"ev100":ev,"tone_mapper":"PBR Neutral","samples":samples,"surface_roi_convergence":surface_roi_convergence,"bounces":12,"settle_frames":256,"error":error,"convergence":convergence}));
        reference.destroy(gpu)?;
    }
    write_json(
        &output_path(root, "measurements/robust-quality.json")?,
        &report,
    )?;
    gpu.assert_clean()?;
    anyhow::ensure!(
        passed,
        "Robust scene radiance target failed; see robust-quality.json"
    );
    Ok(())
}

/// Measure complete native/upscaled frames; new fixtures have no historical baseline.
pub fn timings(gpu: &mut GpuDevice, root: &Path, night_only: bool) -> Result<()> {
    let mut report = Vec::new();
    let mut budgets_passed = true;
    for (name, scene, camera, ev) in cases()? {
        if night_only && name != "night_interior" {
            continue;
        }
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
            let background_before = content::background_load();
            let cpu_before = content::cpu_load();
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
            view.capture(gpu)?.save(output_path(
                root,
                format!("captures/{name}-{}p.png", render[1]),
            )?)?;
            let frame_samples = totals.to_vec();
            totals.sort_by(f64::total_cmp);
            let budget = if render[1] == 1440 {
                10.
            } else if name == "night_interior" {
                // Accepted round-2 p95 plus the explicitly allowed 1 ms increment.
                3.533664 + 1.
            } else {
                5.
            };
            budgets_passed &= totals[56] <= budget;
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
            report.push(serde_json::json!({"scene":name,"render_size":render,"display_size":[2560,1440],"p50":totals[30],"p95":totals[56],"maximum_ms":totals.last(),"frame_samples_ms":frame_samples,"budget_ms":budget,"budget_passed":totals[56]<=budget,"warmup":96,"frames":60,"passes":passes,"exposure":exposure,"scene_hash":view.scene_hash(),"shader_revision":view.shader_revision(),"triangles":scene.meshes.iter().map(|m|m.indices.len()/3).sum::<usize>(),"background_before":background_before,"background_after":content::background_load(),"cpu_before":cpu_before,"cpu_after":content::cpu_load(),"background_load":"shared GPU with approximately ten other agents; no forced clocks or exclusive access","async_overlap":false}));
            view.destroy(gpu)?;
        }
    }
    let destination = if night_only {
        "measurements/night-trial-timings.json"
    } else {
        "measurements/robust-timings.json"
    };
    write_json(&output_path(root, destination)?, &report)?;
    write_json(
        &output_path(root, "measurements/validation.json")?,
        &gpu.diagnostics(),
    )?;
    gpu.assert_clean()?;
    anyhow::ensure!(
        budgets_passed,
        "Robust scene p95 budget failed; see robust-timings.json"
    );
    Ok(())
}
