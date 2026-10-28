//! Identical 1M-sprite workloads at two world diameters, with measured coverage.
use anyhow::{Result, ensure};
use ara_gpu::GpuDevice;
use ara_render::scene::CameraSpec;
use ara_vfx_gpu::{GpuSystem, View};
use glam::Vec3;
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Serialize)]
struct StageTiming {
    pass: String,
    p50_ms: f64,
    p95_ms: f64,
}
#[derive(Serialize)]
struct BenchmarkReport {
    sprite_diameter_m: f32,
    async_compute: bool,
    resolution: [u32; 2],
    warmup: u32,
    samples: u32,
    stats: ara_vfx_gpu::Stats,
    stages: Vec<StageTiming>,
    camera: CameraSpec,
    box_size_m: [f32; 3],
    coverage_budget_layers: u32,
    niagara_comparison: &'static str,
}
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = GpuDevice::new(&[], root.join(".cache"))?;
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
                ara_vfx_gpu::LINEAR_HDR,
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
            let stages = samples
                .into_iter()
                .map(|(pass, mut ms)| {
                    ms.sort_by(f64::total_cmp);
                    StageTiming {
                        pass,
                        p50_ms: ms[ms.len() / 2],
                        p95_ms: ms[ms.len() * 95 / 100],
                    }
                })
                .collect();
            println!(
                "{diameter} m, async={async_compute}: {} alive, {} drawn, {:.3} estimated layers",
                stats.particles_alive, stats.emitters[0].rendered, stats.overdraw_estimate
            );
            results.push(BenchmarkReport {
                sprite_diameter_m: diameter,
                async_compute,
                resolution: [2560, 1440],
                warmup: 30,
                samples: 120,
                stats,
                stages,
                camera: view.camera,
                box_size_m: [6., 3.6, 1.6],
                coverage_budget_layers: 64,
                niagara_comparison: "See COMPARISON.md; published workloads are not matched benchmarks.",
            });
            system.retire(&mut gpu)?;
        }
    }
    std::fs::write(
        root.join("captures/timings.json"),
        ara_json::to_canonical_string(&ara_json::to_value(&results)?),
    )?;
    gpu.assert_clean()?;
    Ok(())
}
