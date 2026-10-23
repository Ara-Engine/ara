use super::*;
use ara_pathtrace::metrics;

/// Enforce RMSE and reference-convergence limits on the original views.
pub fn run(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let camera = scenes::camera();
    let mut passed = true;
    let mut report = Vec::new();
    std::fs::create_dir_all(root.join(".sn/quality"))?;
    for (name, target) in [
        ("glossy_interior", 0.08),
        ("thin_wall", 0.1),
        ("furnace", 0.025),
    ] {
        let scene = scenes::scene(name, 0);
        let mut s = settings([320, 180], [320, 180]);
        if name == "furnace" {
            s.post.ev100 = Some(0.);
        }
        let mut view = FidelityView::new(gpu, &scene, s, None)?;
        for tick in 0..96 {
            view.render(gpu, &input(&camera, tick, false))?;
        }
        let hybrid = view.capture_radiance(gpu)?;
        view.capture(gpu)?
            .save(root.join(format!(".sn/quality/{name}-quality.png")))?;
        view.destroy(gpu)?;
        let mut reference = Reference::new(gpu, &scene, [320, 180], 4096, 12, s.post)?;
        for _ in 0..256 {
            reference.step(gpu, &camera, false)?;
        }
        let half = reference.radiance(gpu)?;
        reference.converge(gpu, &camera)?;
        let ground = reference.radiance(gpu)?;
        reference
            .capture(gpu)?
            .save(root.join(format!(".sn/quality/{name}-ground.png")))?;
        let error = metrics::radiance(&hybrid, &ground)?;
        let convergence = metrics::radiance(&half, &ground)?;
        println!(
            "{name}: relative RMSE {:.5}, target {target}; reference residual {:.5}",
            error.relative_rmse, convergence.relative_rmse
        );
        report.push(serde_json::json!({"scene":name,"relative_rmse":error.relative_rmse,"target":target,"reference_samples":4096,"reference_bounces":12,"reference_precision":"RGBA32F accumulation, unfiltered","reference_convergence":convergence.relative_rmse,"hybrid_settle_frames":96}));
        anyhow::ensure!(
            convergence.relative_rmse < 0.03,
            "Reference has not converged (or accumulation lost precision)"
        );
        for (kind, data) in [("hybrid", &hybrid), ("ground", &ground)] {
            let bytes = data
                .iter()
                .flatten()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>();
            std::fs::write(root.join(format!(".sn/quality/{name}-{kind}.f32")), bytes)?;
        }
        passed &= error.relative_rmse <= target;
        reference.destroy(gpu)?;
    }
    ara_gpu::cooked::write_json(&root.join("measurements/quality-targets.json"), &report)?;
    gpu.assert_clean()?;
    anyhow::ensure!(passed, "Quality targets failed");
    Ok(())
}

#[test]
fn quality_targets_against_independent_4096_sample_reference() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".sn/quality-test-cache"))?;
    run(&mut gpu, root)
}
