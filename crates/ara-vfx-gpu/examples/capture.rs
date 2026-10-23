use anyhow::Result;
use ara_gpu::vulkan::VulkanDevice;
use ara_render::capture::{CaptureRequest, ViewMode};
use ara_vfx_gpu::{GpuSystem, View};
use std::path::Path;

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("captures");
    std::fs::create_dir_all(&output)?;
    let mut gpu = VulkanDevice::new(&[], root.join(".cache"))?;
    let selected = std::env::args().nth(1);
    for (effect, tick) in ara_vfx::samples::all() {
        if selected.as_ref().is_some_and(|name| name != &effect.name) {
            continue;
        }
        let mut system = GpuSystem::new(
            &mut gpu,
            &effect,
            &root.join("cooked").join(&effect.name),
            [960, 540],
        )?;
        let request = CaptureRequest {
            camera: View::default().camera,
            size: [960, 540],
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: 1.,
            settle_frames: 1,
            tick: u64::from(tick),
            seed: u64::from(effect.seed),
            marks: None,
        };
        let capture = system.capture(&mut gpu, &request)?;
        capture.write(&output.join(format!("{}.png", effect.name)))?;
        let stats = system.stats(&gpu)?;
        std::fs::write(
            output.join(format!("{}.stats.json", effect.name)),
            ara_json::to_canonical_string(&serde_json::to_value(&stats)?),
        )?;
        println!(
            "{}: {} alive; {} batches",
            effect.name, stats.particles_alive, stats.batches
        );
        system.retire(&mut gpu)?;
    }
    gpu.context().assert_clean()?;
    Ok(())
}
