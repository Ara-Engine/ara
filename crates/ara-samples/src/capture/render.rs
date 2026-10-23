//! Fidelity captures and timings: each look shot at 2560×1440 native (TAA) and
//! 1707×960 → 2560×1440 (FSR1), a camera-move strip, and per-pass GPU timing
//! distributions with the shared-GPU background load recorded around them.
use super::{
    convert::Stats,
    look::{Look, Shot, Sky},
};
use anyhow::{Context, Result};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::scene::{CameraSpec, Scene};
use ara_render_fidelity::{
    FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, ToneMapper, Upscaler,
    atmosphere::Atmosphere,
};
use image::{RgbaImage, imageops};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// Display resolution of every capture.
pub const DISPLAY: [u32; 2] = [2560, 1440];
/// Transport resolution of the upscaled mode (2/3 scale).
pub const UPSCALED: [u32; 2] = [1707, 960];

/// Where captures go and how much to measure.
pub struct Options<'a> {
    /// Evidence directory (small PNGs and timing JSON, committed).
    pub evidence: &'a Path,
    /// Scratch directory (full-resolution captures, ignored).
    pub scratch: &'a Path,
    /// File-name prefix (`outpost`).
    pub name: &'a str,
    /// Width of evidence PNGs.
    pub evidence_width: u32,
    /// Frames to settle a shot before capturing.
    pub settle: u32,
    /// Frames in each timing window (after the settle).
    pub timing_frames: u32,
    /// Only the first shot (quick look iterations).
    pub quick: bool,
    /// The scene's visual effects, composited into every capture.
    pub effects: &'a [ara_runtime::extract::EffectView],
}

fn camera(shot: &Shot) -> CameraSpec {
    CameraSpec::LookAt {
        position: glam::Vec3::from_array(shot.position),
        target: glam::Vec3::from_array(shot.target),
        vertical_fov: shot.fov.to_radians(),
    }
}

fn post(look: &Look) -> PostSettings {
    PostSettings {
        ev100: look.post.ev100,
        compensation: look.post.compensation,
        bloom: look.post.bloom,
        fog_density: look.post.fog_density,
        tone_mapper: if look.post.neutral {
            ToneMapper::PbrNeutral
        } else {
            ToneMapper::AcesFit
        },
        ..PostSettings::default()
    }
}

fn configure(gpu: &mut VulkanDevice, view: &mut FidelityView, look: &Look) -> Result<()> {
    if look.reservoirs {
        view.set_gather_events(2)?;
        view.set_transport_stride(2)?;
        view.set_reservoir_sampling(gpu, true)?;
    } else {
        view.set_gather_events(1)?;
        view.set_transport_stride(4)?;
    }
    if let Sky::Atmosphere {
        fog_extinction,
        fog_height,
        fog_albedo,
        cloud_coverage,
        sea_level,
    } = look.sky
    {
        view.set_atmosphere(Some(Atmosphere {
            sea_level,
            fog_extinction,
            fog_height,
            fog_albedo,
            cloud_coverage,
            cloud_height: 2200.0,
        }))?;
    }
    Ok(())
}

fn frame(camera: &CameraSpec, tick: u32, reset: bool) -> FrameInput<'_> {
    FrameInput {
        camera,
        tick,
        delta_seconds: 1.0 / 60.0,
        reset,
        lighting_changed: reset,
        deterministic: false,
    }
}

/// `nvidia-smi` utilisation/memory of the whole GPU: this process plus any other
/// agents sharing it, so it shows contention only when this view is idle.
pub fn gpu_load() -> String {
    std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu,memory.used,memory.total,power.draw,clocks.gr",
            "--format=csv,noheader",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|e| format!("unavailable: {e}"))
}

/// Whole-GPU memory in use (MiB) per `nvidia-smi`. WDDM does not attribute
/// graphics memory per process, so deltas around a view include other agents' work.
pub fn gpu_memory() -> Option<f64> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn save_small(image: &RgbaImage, width: u32, path: &Path) -> Result<()> {
    let height = (u64::from(width) * u64::from(image.height()) / u64::from(image.width())) as u32;
    imageops::resize(image, width, height, imageops::FilterType::Lanczos3)
        .save(path)
        .with_context(|| format!("saving {}", path.display()))
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// Renders every shot in both modes, the strip and the timing windows; writes the
/// evidence PNGs and `<name>-timings.json`. Returns the report.
pub fn run(
    gpu: &mut VulkanDevice,
    scene: &Scene,
    stats: &Stats,
    look: &Look,
    options: &Options<'_>,
) -> Result<Value> {
    std::fs::create_dir_all(options.evidence)?;
    std::fs::create_dir_all(options.scratch)?;
    let shots: Vec<&Shot> = if options.quick {
        look.shots.iter().take(1).collect()
    } else {
        look.shots.iter().collect()
    };
    let mut modes = Vec::new();
    for (label, render, upscaler) in [
        ("native", DISPLAY, Upscaler::Taau),
        ("upscaled", UPSCALED, Upscaler::Fsr1),
    ] {
        if options.quick && label == "upscaled" {
            continue;
        }
        let memory_before = gpu_memory();
        let created = std::time::Instant::now();
        let mut view = FidelityView::new(
            gpu,
            scene,
            Settings {
                render_size: render,
                display_size: DISPLAY,
                tier: Tier::Rt,
                mode: Mode::Hybrid,
                upscaler,
                probe_ray_budget: 16384,
                reflection_cutoff: 0.6,
                post: post(look),
            },
            None,
        )?;
        configure(gpu, &mut view, look)?;
        let mut effects = ara_runtime_render::fidelity::vfx::Effects::new(
            gpu,
            options.effects,
            render,
            (scene.sun_direction, scene.sun_lux),
        )?;
        let create_ms = created.elapsed().as_secs_f64() * 1000.0;
        let mut tick = 0;
        let mut timing = Value::Null;
        for (i, shot) in shots.iter().enumerate() {
            let cam = camera(shot);
            for n in 0..options.settle {
                view.render(gpu, &frame(&cam, tick, n == 0))?;
                tick += 1;
            }
            if i == 0 {
                let before = gpu_load();
                let mut totals = Vec::new();
                let mut passes: BTreeMap<String, Vec<f64>> = BTreeMap::new();
                for _ in 0..options.timing_frames {
                    view.render(gpu, &frame(&cam, tick, false))?;
                    tick += 1;
                    totals.push(view.timings().iter().map(|p| p.milliseconds).sum::<f64>());
                    for p in view.timings() {
                        passes
                            .entry(p.pass.to_owned())
                            .or_default()
                            .push(p.milliseconds);
                    }
                }
                let samples = totals.clone();
                totals.sort_by(f64::total_cmp);
                let passes: BTreeMap<String, Value> = passes
                    .into_iter()
                    .map(|(k, mut v)| {
                        v.sort_by(f64::total_cmp);
                        (
                            k,
                            json!({"p50": percentile(&v, 0.5), "p95": percentile(&v, 0.95)}),
                        )
                    })
                    .collect();
                println!(
                    "{} {label} {}x{}: p50 {:.2} ms, p95 {:.2} ms ({} frames)",
                    options.name,
                    render[0],
                    render[1],
                    percentile(&totals, 0.5),
                    percentile(&totals, 0.95),
                    totals.len()
                );
                timing = json!({
                    "shot": shot.name,
                    "frames": totals.len(),
                    "p50": percentile(&totals, 0.5),
                    "p95": percentile(&totals, 0.95),
                    "max": totals.last(),
                    "frameSamplesMs": samples,
                    "passes": passes,
                    "gpuBefore": before,
                    "gpuAfter": gpu_load(),
                });
            }
            if !effects.is_empty() {
                effects.composite(gpu, &mut view, &cam, tick)?;
            }
            let image = view.capture(gpu)?;
            image.save(
                options
                    .scratch
                    .join(format!("{}-{}-{label}.png", options.name, shot.name)),
            )?;
            if label == "native" {
                save_small(
                    &image,
                    options.evidence_width,
                    &options
                        .evidence
                        .join(format!("{}-{}.png", options.name, shot.name)),
                )?;
            } else if i == 0 {
                save_small(
                    &image,
                    options.evidence_width,
                    &options
                        .evidence
                        .join(format!("{}-{}-upscaled.png", options.name, shot.name)),
                )?;
            }
        }
        if label == "native"
            && let Some(sequence) = &look.sequence
            && !options.quick
        {
            strip(
                gpu,
                &mut view,
                &mut effects,
                look,
                sequence,
                &mut tick,
                options,
            )?;
        }
        let exposure = view.capture_exposure(gpu).ok().map(|e| format!("{e:?}"));
        modes.push(json!({
            "mode": label,
            "renderSize": render,
            "displaySize": DISPLAY,
            "upscaler": format!("{upscaler:?}"),
            "viewCreateMs": create_ms,
            "gpuMemoryUsedMiB": {"beforeView": memory_before, "afterCaptures": gpu_memory(), "note": "whole GPU (shared); the delta approximates this view"},
            "timing": timing,
            "exposure": exposure,
            "sceneHash": view.scene_hash(),
            "shaderRevision": view.shader_revision(),
        }));
        effects.destroy(gpu)?;
        view.destroy(gpu)?;
    }
    let report = json!({
        "scene": look.scene,
        "name": options.name,
        "budgetMs": {"native1440p": 16.0, "upscaled": 8.0},
        "content": stats,
        "modes": modes,
        "sharedGpu": "other agents share this RTX 4070 SUPER; gpuBefore/gpuAfter record nvidia-smi load around each window",
    });
    let mut text = serde_json::to_string_pretty(&report)?;
    text.push('\n');
    std::fs::write(
        options
            .evidence
            .join(format!("{}-timings.json", options.name)),
        text,
    )?;
    Ok(report)
}

/// A camera move from one shot to another, captured into a horizontal strip.
fn strip(
    gpu: &mut VulkanDevice,
    view: &mut FidelityView,
    effects: &mut ara_runtime_render::fidelity::vfx::Effects,
    look: &Look,
    sequence: &super::look::Sequence,
    tick: &mut u32,
    options: &Options<'_>,
) -> Result<()> {
    let find = |name: &str| {
        look.shots
            .iter()
            .find(|s| s.name == name)
            .with_context(|| format!("no shot {name}"))
    };
    let (a, b) = (find(&sequence.from)?, find(&sequence.to)?);
    let lerp = |x: [f32; 3], y: [f32; 3], t: f32| [0, 1, 2].map(|i| x[i] + (y[i] - x[i]) * t);
    let (w, h) = (480u32, 270u32);
    let mut out = RgbaImage::new(w * sequence.frames, h);
    let steps = sequence.frames.saturating_sub(1).max(1) * sequence.step;
    let mut frame_index = 0;
    let start = camera(a);
    for n in 0..options.settle {
        view.render(gpu, &frame(&start, *tick, n == 0))?;
        *tick += 1;
    }
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        // Smoothstep ease so the strip reads as a dolly, not a cut.
        let t = t * t * (3.0 - 2.0 * t);
        let shot = Shot {
            name: String::new(),
            position: lerp(a.position, b.position, t),
            target: lerp(a.target, b.target, t),
            fov: a.fov + (b.fov - a.fov) * t,
        };
        let cam = camera(&shot);
        view.render(gpu, &frame(&cam, *tick, false))?;
        *tick += 1;
        if s % sequence.step == 0 && frame_index < sequence.frames {
            if !effects.is_empty() {
                effects.composite(gpu, view, &cam, *tick)?;
            }
            let image = view.capture(gpu)?;
            let small = imageops::resize(&image, w, h, imageops::FilterType::Triangle);
            imageops::overlay(&mut out, &small, i64::from(frame_index * w), 0);
            frame_index += 1;
        }
    }
    out.save(
        options
            .evidence
            .join(format!("{}-sequence.png", options.name)),
    )?;
    Ok(())
}
