mod accel;
mod gpu;
mod passes;
mod pipelines;
mod render;
mod scene;
mod window;

use anyhow::{Context, Result, bail, ensure};
use std::sync::atomic::Ordering;

struct Options {
    window: bool,
    frames: u32,
    shadows: bool,
    output: String,
    evidence: String,
}

fn options() -> Result<Options> {
    let mut options = Options {
        window: false,
        frames: 120,
        shadows: true,
        output: "RESULT.png".into(),
        evidence: "evidence/run.json".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--window" => options.window = true,
            "--no-shadows" => options.shadows = false,
            "--frames" => {
                options.frames = args.next().context("--frames requires a value")?.parse()?
            }
            "--output" => options.output = args.next().context("--output requires a value")?,
            "--evidence" => {
                options.evidence = args.next().context("--evidence requires a value")?
            }
            _ => bail!("Unknown argument: {arg}"),
        }
    }
    ensure!(options.frames > 0, "At least one frame required");
    Ok(options)
}

fn main() -> Result<()> {
    let options = options()?;
    std::fs::create_dir_all("tools")?;
    std::fs::create_dir_all("evidence")?;
    let window = if options.window {
        Some(window::Window::new()?)
    } else {
        None
    };
    let mut gpu = gpu::Gpu::new(window.as_ref())?;
    let scene = scene::create(&mut gpu)?;
    let acceleration = accel::build(&mut gpu, &scene)?;
    let pipelines = pipelines::Pipelines::create(&gpu, &scene, acceleration.tlas)?;
    let result = render::run(
        &mut gpu,
        &scene,
        &pipelines,
        window.as_ref(),
        options.frames,
        options.shadows,
        &options.output,
    );
    pipelines.destroy(&gpu);
    let result = result?;
    gpu.assert_clean()?;
    let evidence = serde_json::json!({
        "device": gpu.info,
        "validation": { "layer": "VK_LAYER_KHRONOS_validation", "synchronization_validation": true,
            "errors": gpu.validation.errors.load(Ordering::Relaxed), "warnings": gpu.validation.warnings.load(Ordering::Relaxed),
            "messages": *gpu.validation.messages.lock().map_err(|e| anyhow::anyhow!("Validation lock: {e}"))? },
        "scene": { "width": render::WIDTH, "height": render::HEIGHT, "materials": scene.draws.len(), "textures": scene.textures.len(), "triangles": acceleration.triangles, "shadows": options.shadows },
        "acceleration_build_gpu_ms": acceleration.build_ms,
        "cache": pipelines.measurements,
        "bindless": { "global_sets": 1, "sampled_image_capacity": 16, "storage_buffer_capacity": 4,
            "update_after_bind": true, "texture_slot_3_updated_after_command_recording": true },
        "render": result,
        "screenshot": options.output,
    });
    std::fs::write(&options.evidence, serde_json::to_vec_pretty(&evidence)?)?;
    println!(
        "Saved {} and {}; validation clean",
        options.output, options.evidence
    );
    Ok(())
}
