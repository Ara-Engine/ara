//! Deterministic fidelity gallery, mid-life images and four-frame sequence strips.
mod support;
use anyhow::Result;
use ara_gpu::vulkan::VulkanDevice;
use std::path::Path;
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("captures");
    std::fs::create_dir_all(output.join("sequences"))?;
    let mut gpu = VulkanDevice::new(&[], root.join(".cache/gallery"))?;
    let selected = std::env::args().nth(1);
    for (effect, tick) in ara_vfx::samples::all() {
        if selected.as_ref().is_some_and(|name| name != &effect.name) {
            continue;
        }
        let sequence = match effect.name.as_str() {
            "muzzle_flash" => [1, 2, 3, 6],
            "campfire" => [30, 70, 120, 180],
            "sparks" => [10, 40, 100, 160],
            "magic_swirl" => [30, 75, 140, 200],
            "smoke" => [40, 100, 200, 280],
            _ => [30, 60, 100, 160],
        };
        let mut frames = support::capture_frames(&mut gpu, &effect, &sequence, [960, 540])?;
        let mut strip = image::RgbaImage::new(1920, 270);
        for (i, (capture, stats)) in frames.iter_mut().enumerate() {
            capture.write(
                &output
                    .join("sequences")
                    .join(format!("{}-{}.png", effect.name, sequence[i])),
            )?;
            if sequence[i] == tick {
                capture.write(&output.join(format!("{}.png", effect.name)))?;
                std::fs::write(
                    output.join(format!("{}.stats.json", effect.name)),
                    ara_json::to_canonical_string(&ara_json::to_value(stats)?),
                )?;
            }
            let thumbnail = image::imageops::resize(
                &capture.image,
                480,
                270,
                image::imageops::FilterType::Lanczos3,
            );
            image::imageops::replace(&mut strip, &thumbnail, (i * 480) as i64, 0);
        }
        strip.save(output.join(format!("{}.sequence.png", effect.name)))?;
        std::fs::write(
            output.join(format!("{}.sequence.json", effect.name)),
            ara_json::to_canonical_string(
                &serde_json::json!({"ticks":sequence,"hz":effect.hz,"columns":4,"size":[480,270]}),
            ),
        )?;
        println!("{}: four fidelity frames captured", effect.name);
    }
    gpu.context().assert_clean()?;
    Ok(())
}
