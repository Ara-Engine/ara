use super::*;
use ara_gpu::cooked::write_json;
use ara_pathtrace::metrics;
use ara_render::capture::compare;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
struct CaseError {
    scene: String,
    scene_hash: String,
    reference_samples: u32,
    reference_bounces: u32,
    ssim_error: f64,
    radiance: metrics::RadianceError,
    convergence_error: metrics::RadianceError,
}
pub fn captures(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let size = [320, 180];
    let camera = scenes::camera();
    let mut report = Vec::new();
    let update = std::env::var("ARA_UPDATE_FIDELITY_GOLDENS").as_deref() == Ok("1");
    let goldens = root.join("tests/goldens");
    std::fs::create_dir_all(&goldens)?;
    for name in scenes::NAMES {
        println!("Capturing {name}: 96 hybrid settle frames, 4096 reference spp / 12 bounces");
        let scene = scenes::scene(name, 0);
        let mut s = settings(size, size);
        if name == "furnace" {
            s.post.ev100 = Some(0.);
        }
        let mut view = FidelityView::new(gpu, &scene, s, None)?;
        for tick in 0..96 {
            view.render(gpu, &input(&camera, tick, false))?;
        }
        let hybrid = view.capture(gpu)?;
        let hdr = view.capture_radiance(gpu)?;
        hybrid.save(root.join(format!("captures/{name}-hybrid.png")))?;
        let golden = goldens.join(format!("{name}.png"));
        if update {
            hybrid.save(&golden)?;
        }
        let result = compare(&hybrid, &image::open(&golden)?.to_rgba8(), 0.005)?;
        anyhow::ensure!(
            result.passed,
            "{name}: golden 1-SSIM {} exceeds 0.005",
            result.score
        );
        let scene_hash = view.scene_hash().to_owned();
        if name == "glossy_interior" {
            let baked = view.bake_snapshot(gpu)?;
            write_json(&root.join("measurements/baked-interior.json"), &baked)?;
            let mut raster = s;
            raster.tier = Tier::Raster;
            let mut fallback = FidelityView::new(gpu, &scene, raster, Some(&baked))?;
            fallback.render(gpu, &input(&camera, 95, true))?;
            fallback
                .capture(gpu)?
                .save(root.join("captures/baked-raster.png"))?;
            fallback.destroy(gpu)?;
        }
        view.destroy(gpu)?;
        let mut reference = Reference::new(gpu, &scene, size, 4096, 12, s.post)?;
        for _ in 0..256 {
            reference.step(gpu, &camera, false)?;
        }
        let half = reference.radiance(gpu)?;
        reference.converge(gpu, &camera)?;
        let image = reference.capture(gpu)?;
        let ground = reference.radiance(gpu)?;
        image.save(root.join(format!("captures/{name}-reference.png")))?;
        let display = compare(&hybrid, &image, 2.)?;
        display
            .diff
            .save(root.join(format!("captures/{name}-diff.png")))?;
        let error = metrics::radiance(&hdr, &ground)?;
        let convergence = metrics::radiance(&half, &ground)?;
        println!(
            "{name}: 1-SSIM {:.6}, linear relative RMSE {:.4}, reference 2048 -> 4096 relative RMSE {:.4}",
            display.score, error.relative_rmse, convergence.relative_rmse
        );
        report.push(CaseError {
            scene: name.into(),
            scene_hash,
            reference_samples: 4096,
            reference_bounces: 12,
            ssim_error: display.score,
            radiance: error,
            convergence_error: convergence,
        });
        reference.destroy(gpu)?;
    }
    write_json(&root.join("measurements/reference-errors.json"), &report)?;
    let shaders: ara_gpu::cooked::CookedPipeline = serde_json::from_slice(&std::fs::read(
        root.join("../ara-render-fidelity/cooked/primary.json"),
    )?)?;
    write_json(
        &root.join("measurements/capture-provenance.json"),
        &serde_json::json!({"camera":camera,"render_size":size,"display_size":size,"hybrid_settle_frames":96,"hybrid_ticks":[0,95],"reference_samples":4096,"reference_bounces":12,"sampling":"integer hash of pixel and sample/tick; no wall clock","exposure_ev100":{"furnace":0,"other_scenes":3},"golden_ssim_tolerance":0.005,"shaders":shaders}),
    )?;
    write_json(
        &root.join("measurements/validation.json"),
        &gpu.diagnostics(),
    )?;

    sequences(gpu, root)?;
    lag::capture(gpu, root)?;
    write_json(&root.join("measurements/device.json"), &gpu.context().info)?;
    gpu.assert_clean()?;
    Ok(())
}
#[derive(Serialize)]
struct SequenceError {
    scene: String,
    frames: u32,
    maximum_residual: f64,
    mean_residual: f64,
    reset_residual: f64,
}
fn sequences(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let camera = scenes::camera();
    let size = [240, 135];
    let mut reports = Vec::new();
    for name in ["moving_emissive", "time_of_day", "disocclusion"] {
        let mut view = FidelityView::new(gpu, &scenes::scene(name, 0), settings(size, size), None)?;
        let mut fresh_view =
            FidelityView::new(gpu, &scenes::scene(name, 0), settings(size, size), None)?;
        for tick in 0..48 {
            view.render(gpu, &input(&camera, tick, false))?;
            fresh_view.render(gpu, &input(&camera, tick, false))?;
        }
        let mut errors = Vec::new();
        let mut sheet = image::RgbaImage::new(size[0] * 4, size[1] * 3);
        for i in 0..12 {
            let tick = i * 4;
            let scene = scenes::scene(name, tick);
            view.update_scene(gpu, &scene)?;
            fresh_view.update_scene(gpu, &scene)?;
            let mut request = input(&camera, tick, false);
            request.lighting_changed = name != "disocclusion";
            view.render(gpu, &request)?;
            let accumulated = view.capture(gpu)?;
            image::imageops::replace(
                &mut sheet,
                &accumulated,
                i64::from((i % 4) * size[0]),
                i64::from((i / 4) * size[1]),
            );
            request.reset = true;
            fresh_view.render(gpu, &request)?;
            let fresh = fresh_view.capture(gpu)?;
            let error = metrics::temporal_residual(&accumulated, &fresh, |x, y| {
                x > size[0] / 4 && x < size[0] * 3 / 4 && y > size[1] / 5 && y < size[1] * 4 / 5
            })?;
            errors.push(error);
        }
        sheet.save(root.join(format!("captures/{name}-sequence.png")))?;
        let request = input(&camera, 44, true);
        view.render(gpu, &request)?;
        let a = view.capture(gpu)?;
        view.render(gpu, &request)?;
        let b = view.capture(gpu)?;
        let reset = metrics::temporal_residual(&a, &b, |_, _| true)?;
        let maximum = errors.iter().copied().fold(0_f64, f64::max);
        let mean = errors.iter().sum::<f64>() / errors.len() as f64;
        anyhow::ensure!(
            maximum < 0.15 && reset < 0.03,
            "{name}: temporal residual exceeds explicit limits: max {maximum}, reset {reset}"
        );
        println!(
            "{name} sequence: maximum residual {maximum:.5}, mean {mean:.5}, reset {reset:.5}"
        );
        reports.push(SequenceError {
            scene: name.into(),
            frames: 12,
            maximum_residual: maximum,
            mean_residual: mean,
            reset_residual: reset,
        });
        view.destroy(gpu)?;
        fresh_view.destroy(gpu)?;
    }
    write_json(&root.join("measurements/temporal-errors.json"), &reports)?;
    Ok(())
}
#[derive(Serialize)]
struct Distribution {
    p50: f64,
    p95: f64,
    maximum: f64,
}
fn distribution(mut samples: Vec<f64>) -> Distribution {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    Distribution {
        p50: samples[n / 2],
        p95: samples[((n as f64 * 0.95).ceil() as usize).saturating_sub(1)],
        maximum: samples[n - 1],
    }
}
#[derive(Serialize)]
struct Performance {
    render_size: [u32; 2],
    display_size: [u32; 2],
    samples: u32,
    probe_ray_budget: u32,
    upscaler: String,
    validation: bool,
    async_overlap: bool,
    total: Distribution,
    passes: BTreeMap<String, Distribution>,
    budget_ms: f64,
    budget_passed: bool,
}
pub fn timings(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let scene = scenes::scene("glossy_interior", 0);
    let camera = scenes::camera();
    let mut results = Vec::new();
    std::fs::create_dir_all(root.join("measurements"))?;
    for (render, budget) in [([2560, 1440], 8.), ([1707, 960], 5.)] {
        let mut s = settings(render, [2560, 1440]);
        s.post = PostSettings {
            ev100: None,
            ..PostSettings::default()
        };
        let mut view = FidelityView::new(gpu, &scene, s, None)?;
        let mut passes = BTreeMap::<String, Vec<f64>>::new();
        let mut totals = Vec::new();
        for tick in 0..72 {
            let mut request = input(&camera, tick, false);
            request.deterministic = false;
            view.render(gpu, &request)?;
            if tick >= 12 {
                let mut total = 0.;
                for t in view.timings() {
                    passes
                        .entry(t.pass.to_owned())
                        .or_default()
                        .push(t.milliseconds);
                    total += t.milliseconds;
                }
                totals.push(total);
            }
        }
        view.capture(gpu)?
            .save(root.join(format!("captures/interior-{}p.png", render[1])))?;
        let total = distribution(totals);
        println!(
            "{}x{} -> 2560x1440: GPU sum p50 {:.3} ms p95 {:.3} ms, budget {budget}",
            render[0], render[1], total.p50, total.p95
        );
        results.push(Performance {
            render_size: render,
            display_size: [2560, 1440],
            samples: 60,
            probe_ray_budget: s.probe_ray_budget,
            upscaler: if render == [2560, 1440] {
                "Native TAA".into()
            } else {
                "FSR1 EASU/RCAS + engine TAA".into()
            },
            validation: true,
            async_overlap: false,
            budget_passed: total.p95 < budget,
            total,
            passes: passes
                .into_iter()
                .map(|(k, v)| (k, distribution(v)))
                .collect(),
            budget_ms: budget,
        });
        view.destroy(gpu)?;
    }
    write_json(&root.join("measurements/gpu-timings.json"), &results)?;
    write_json(&root.join("measurements/device.json"), &gpu.context().info)?;
    gpu.assert_clean()?;
    Ok(())
}
