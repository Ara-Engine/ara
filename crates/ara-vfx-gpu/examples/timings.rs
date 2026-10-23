use anyhow::{Result, ensure};
use ara_gpu::vulkan::VulkanDevice;
use ara_vfx_gpu::{GpuSystem, View};
use std::{collections::BTreeMap, path::Path};

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let effect = ara_vfx::samples::million();
    let mut gpu = VulkanDevice::new(&[], root.join(".cache"))?;
    let mut results = Vec::new();
    for async_compute in [false, true] {
        let mut system = GpuSystem::new(
            &mut gpu,
            &effect,
            &root.join("cooked/million"),
            [2560, 1440],
        )?;
        let view = View {
            async_compute,
            ..View::default()
        };
        for _ in 0..30 {
            system.step(&mut gpu, &view)?;
        }
        let mut samples = BTreeMap::<String, Vec<f64>>::new();
        for _ in 0..120 {
            system.step(&mut gpu, &view)?;
            let stats = system.stats(&gpu)?;
            for stage in stats.gpu_stages {
                samples
                    .entry(stage.pass)
                    .or_default()
                    .push(stage.milliseconds);
            }
        }
        let stats = system.stats(&gpu)?;
        ensure!(
            stats.particles_alive == 1_000_000 && stats.emitters[0].rendered == 1_000_000,
            "million-particle benchmark must update and draw every particle"
        );
        let stages=samples.into_iter().map(|(pass,mut ms)|{ms.sort_by(f64::total_cmp);serde_json::json!({"pass":pass,"p50_ms":ms[ms.len()/2],"p95_ms":ms[ms.len()*95/100]})}).collect::<Vec<_>>();
        results.push(serde_json::json!({"async_compute":async_compute,"resolution":[2560,1440],"warmup":30,"samples":120,"stats":stats,"stages":stages,"niagara_baseline":null,"comparison":"Niagara equivalent has not been measured on this machine."}));
        println!("async={async_compute}: {} alive", stats.particles_alive);
        system.retire(&mut gpu)?;
    }
    let path = root.join("captures/timings.json");
    std::fs::create_dir_all(path.parent().expect("capture path has a parent"))?;
    std::fs::write(
        &path,
        ara_json::to_canonical_string(&serde_json::json!(results)),
    )?;
    println!("{}", path.display());
    Ok(())
}
