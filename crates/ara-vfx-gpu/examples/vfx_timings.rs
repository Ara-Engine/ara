//! Identical 1M-sprite workloads at two world diameters, with measured coverage.
use anyhow::{Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::scene::CameraSpec;
use ara_vfx_gpu::{GpuSystem, View};
use glam::Vec3;
use std::{collections::BTreeMap, path::Path};
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join(".cache"))?;
    let mut results = Vec::new();
    for (effect, diameter) in [
        (ara_vfx::samples::million(), 0.003_f32),
        (ara_vfx::samples::million_fill(), 0.05),
    ] {
        for async_compute in [false, true] {
            let mut system = GpuSystem::new(
                &mut gpu,
                &effect,
                &root.join("cooked").join(&effect.name),
                [2560, 1440],
            )?;
            let view = View {
                async_compute,
                camera: CameraSpec::LookAt {
                    position: Vec3::new(0., 2.5, 8.),
                    target: Vec3::new(0., 2.5, 0.),
                    vertical_fov: 0.75,
                },
                ..View::default()
            };
            for _ in 0..30 {
                system.step(&mut gpu, &view)?;
            }
            let mut samples = BTreeMap::<String, Vec<f64>>::new();
            for _ in 0..120 {
                system.step(&mut gpu, &view)?;
                let stats = system.stats(&gpu)?;
                ensure!(
                    stats.particles_alive == 1_000_000 && stats.emitters[0].rendered == 1_000_000,
                    "benchmark must update and submit every particle"
                );
                for stage in stats.gpu_stages {
                    samples
                        .entry(stage.pass)
                        .or_default()
                        .push(stage.milliseconds);
                }
            }
            let stats = system.stats(&gpu)?;
            let stages=samples.into_iter().map(|(pass,mut ms)|{ms.sort_by(f64::total_cmp);serde_json::json!({"pass":pass,"p50_ms":ms[ms.len()/2],"p95_ms":ms[ms.len()*95/100]})}).collect::<Vec<_>>();
            results.push(serde_json::json!({"sprite_diameter_m":ara_json::to_value(&diameter)?,"async_compute":async_compute,"resolution":[2560,1440],"warmup":30,"samples":120,"stats":ara_json::to_value(&stats)?,"stages":stages,"camera":ara_json::to_value(&view.camera)?,"box_size_m":[6,3.6,1.6],"coverage_budget_layers":64,"niagara_comparison":"See COMPARISON.md; published workloads are not matched benchmarks."}));
            println!(
                "{diameter} m, async={async_compute}: {} alive, {} drawn, {:.3} estimated layers",
                stats.particles_alive, stats.emitters[0].rendered, stats.overdraw_estimate
            );
            system.retire(&mut gpu)?;
        }
    }
    std::fs::write(
        root.join("captures/timings.json"),
        ara_json::to_canonical_string(&ara_json::to_value(&results)?),
    )?;
    gpu.context().assert_clean()?;
    Ok(())
}
