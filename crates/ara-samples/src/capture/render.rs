//! Captures and timings per quality preset. Every shot is displayed at 2560×1440
//! through the preset's renderer and transport resolution (Low: the baseline raster
//! renderer with TAA; Medium and High: TAAU from half and two-thirds resolution;
//! Epic: native), plus a camera-move strip per fidelity preset and per-pass GPU
//! timing distributions with the shared-GPU background load recorded around them.
use super::{
    convert::Stats,
    look::{Look, Shot},
    reference::{Truth, score, transport},
};
use anyhow::{Context, Result};
use ara_gpu::vulkan::VulkanDevice;
use ara_render::{
    capture::{CaptureRequest, ViewMode},
    renderer::post::ToneCurve,
    scene::{CameraSpec, Scene},
};
use ara_render_fidelity::{
    FidelityView, FrameInput, PostSettings, ToneMapper,
    environment::FrameEnvironment,
    presets::{Capabilities, FidelityPlan, Plan, QualityChoice, QualityPreset, RenderPath},
};
use image::{RgbaImage, imageops};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// Display resolution of every capture.
pub const DISPLAY: [u32; 2] = [2560, 1440];
/// Frames composited with VFX before a still.
const VFX_FRAMES: u32 = 24;
/// Frames rendered at the reference exposure before scoring.
const SCORE_FRAMES: u32 = 24;

/// Where captures go and how much to measure.
pub struct Options<'a> {
    /// Evidence directory (small PNGs and timing JSON).
    pub evidence: &'a Path,
    /// Scratch directory (full-resolution captures).
    pub scratch: &'a Path,
    /// File-name prefix (`outpost`).
    pub name: &'a str,
    /// Width of evidence PNGs.
    pub evidence_width: u32,
    /// Frames to settle a shot before capturing.
    pub settle: u32,
    /// Frames in each timing window (after the settle).
    pub timing_frames: u32,
    /// Only the first shot, no strip (quick look iterations).
    pub quick: bool,
    /// The scene's visual effects, composited into every fidelity capture.
    pub effects: &'a [ara_runtime::extract::EffectView],
    /// Presets to capture, each into its own file set.
    pub presets: &'a [QualityPreset],
    /// The scene's resolved environment (with any look overrides applied).
    pub environment: &'a FrameEnvironment,
    /// Pipeline cache directory for the baseline renderer's own device.
    pub pipeline_cache: &'a Path,
    /// Reference samples per pixel for RMSE/FLIP scoring; zero skips it.
    pub reference_samples: u32,
}

pub(super) fn camera(shot: &Shot) -> CameraSpec {
    CameraSpec::LookAt {
        position: glam::Vec3::from_array(shot.position),
        target: glam::Vec3::from_array(shot.target),
        vertical_fov: shot.fov.to_radians(),
    }
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

/// Percentiles of whole frames and of each pass over a timing window.
#[derive(Default)]
struct Window {
    totals: Vec<f64>,
    passes: BTreeMap<String, Vec<f64>>,
}
impl Window {
    fn push(&mut self, timings: &[ara_gpu::vulkan::PassTiming]) {
        self.totals
            .push(timings.iter().map(|p| p.milliseconds).sum::<f64>());
        for p in timings {
            self.passes
                .entry(p.pass.to_owned())
                .or_default()
                .push(p.milliseconds);
        }
    }
    fn report(mut self, label: &str, size: [u32; 2], shot: &str, before: String) -> Value {
        let samples = self.totals.clone();
        self.totals.sort_by(f64::total_cmp);
        let passes: BTreeMap<String, Value> = self
            .passes
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
            "{label} {}x{}: p50 {:.2} ms, p95 {:.2} ms ({} frames)",
            size[0],
            size[1],
            percentile(&self.totals, 0.5),
            percentile(&self.totals, 0.95),
            self.totals.len()
        );
        json!({
            "shot": shot,
            "frames": self.totals.len(),
            "p50": percentile(&self.totals, 0.5),
            "p95": percentile(&self.totals, 0.95),
            "max": self.totals.last(),
            "frameSamplesMs": samples,
            "passes": passes,
            "gpuBefore": before,
            "gpuAfter": gpu_load(),
        })
    }
}

/// Renders every shot for every requested preset; writes the evidence PNGs
/// (`<name>-<preset>-<shot>.png`) and `<name>-timings.json`. Returns the report.
/// Fidelity presets run first; Low then consumes the scene for the baseline device.
pub fn run(
    gpu: &mut VulkanDevice,
    scene: Scene,
    stats: &Stats,
    look: &Look,
    options: &Options<'_>,
) -> Result<Value> {
    std::fs::create_dir_all(options.evidence)?;
    std::fs::create_dir_all(options.scratch)?;
    let capabilities = Capabilities::of(gpu);
    let truths = if options.reference_samples > 0 {
        super::reference::render(
            gpu,
            &scene,
            &shots(look, options),
            options.reference_samples,
            options,
        )?
    } else {
        Vec::new()
    };
    let mut modes = Vec::new();
    let mut low = None;
    for &preset in options.presets {
        let plan = Plan::resolve(QualityChoice::Preset(preset), &capabilities, DISPLAY)?;
        match plan.path {
            RenderPath::Fidelity(fidelity_plan) => modes.push(fidelity(
                gpu,
                &scene,
                look,
                options,
                plan.preset.name(),
                &fidelity_plan,
                &truths,
            )?),
            RenderPath::Baseline { render_size } => low = Some(render_size),
        }
    }
    if let Some(size) = low {
        modes.push(baseline(scene, look, options, size, &truths)?);
    }
    let report = json!({
        "scene": look.scene,
        "name": options.name,
        "budgetMs": {"high": {"outdoor": 8.0, "interior": 10.0}, "epic": 16.0},
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

/// The look's shots: the first only with `--quick`, or those named in the
/// comma-separated `ARA_CAPTURE_SHOTS` (look iteration).
fn shots<'a>(look: &'a Look, options: &Options<'_>) -> Vec<&'a Shot> {
    if let Ok(names) = std::env::var("ARA_CAPTURE_SHOTS") {
        let names: Vec<&str> = names.split(',').collect();
        look.shots
            .iter()
            .filter(|s| names.contains(&s.name.as_str()))
            .collect()
    } else if options.quick {
        look.shots.iter().take(1).collect()
    } else {
        look.shots.iter().collect()
    }
}

fn save(image: &RgbaImage, options: &Options<'_>, label: &str, shot: &str) -> Result<()> {
    image.save(
        options
            .scratch
            .join(format!("{}-{label}-{shot}.png", options.name)),
    )?;
    save_small(
        image,
        options.evidence_width,
        &options
            .evidence
            .join(format!("{}-{label}-{shot}.png", options.name)),
    )
}

fn fidelity(
    gpu: &mut VulkanDevice,
    scene: &Scene,
    look: &Look,
    options: &Options<'_>,
    label: &str,
    plan: &FidelityPlan,
    truths: &[Truth],
) -> Result<Value> {
    let memory_before = gpu_memory();
    let created = std::time::Instant::now();
    let mut view = FidelityView::new(
        gpu,
        scene,
        plan.settings(options.environment.post(PostSettings::default())),
        None,
    )?;
    plan.configure(gpu, &mut view)?;
    // Experiment switch: force ReSTIR on or off regardless of the preset's policy.
    if let Some(value) = std::env::var_os("ARA_CAPTURE_RESERVOIRS") {
        view.set_reservoir_sampling(gpu, value == "on")?;
    }
    view.set_environment(options.environment)?;
    let render = plan.render_size;
    let mut effects = ara_runtime_render::fidelity::vfx::Effects::new(
        gpu,
        options.effects,
        render,
        (
            scene.sun_direction,
            options
                .environment
                .ground_sun(scene.sun_direction, scene.sun_lux),
        ),
    )?;
    let create_ms = created.elapsed().as_secs_f64() * 1000.0;
    let mut tick = 0;
    let mut timing = Value::Null;
    let mut scores = Vec::new();
    for (i, shot) in shots(look, options).into_iter().enumerate() {
        let cam = camera(shot);
        for n in 0..options.settle {
            view.render(gpu, &frame(&cam, tick, n == 0))?;
            tick += 1;
        }
        if i == 0 {
            let before = gpu_load();
            let mut window = Window::default();
            for _ in 0..options.timing_frames {
                view.render(gpu, &frame(&cam, tick, false))?;
                tick += 1;
                window.push(view.timings());
            }
            timing = window.report(
                &format!("{} {label}", options.name),
                render,
                &shot.name,
                before,
            );
        }
        // Composite effects into the last frames so the temporal resolve converges
        // on them (one composite alone would show them at the history weight).
        if !effects.is_empty() {
            for _ in 0..VFX_FRAMES {
                view.render(gpu, &frame(&cam, tick, false))?;
                effects.composite(gpu, &mut view, &cam, tick)?;
                tick += 1;
            }
        }
        save(&view.capture(gpu)?, options, label, &shot.name)?;
        if std::env::var_os("ARA_CAPTURE_SIGNALS").is_some() {
            signals(gpu, &view, options, label, &shot.name)?;
        }
        if let Some(truth) = truths.iter().find(|t| t.shot == shot.name) {
            // Score without VFX at the reference's exposure, then restore metering.
            let post = view.settings().post;
            view.set_post(PostSettings {
                ev100: Some(truth.ev100),
                compensation: 0.,
                ..post
            })?;
            for _ in 0..SCORE_FRAMES {
                view.render(gpu, &frame(&cam, tick, false))?;
                tick += 1;
            }
            let heatmap = options
                .scratch
                .join(format!("{}-{label}-{}-flip.png", options.name, shot.name));
            let mut entry = score(&view.capture(gpu)?, truth, &heatmap)?;
            entry["linear"] = transport(&view.capture_radiance(gpu)?, plan.render_size, truth)?;
            scores.push(entry);
            view.set_post(post)?;
        }
    }
    if let Some(sequence) = &look.sequence
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
            label,
        )?;
    }
    let exposure = view.capture_exposure(gpu).ok().map(|e| format!("{e:?}"));
    let report = json!({
        "preset": label,
        "renderer": "fidelity",
        "renderSize": render,
        "displaySize": plan.display_size,
        "plan": format!("{plan:?}"),
        "reservoirs": view.reservoir_sampling(),
        "viewCreateMs": create_ms,
        "gpuMemoryUsedMiB": {"beforeView": memory_before, "afterCaptures": gpu_memory(), "note": "whole GPU (shared); the delta approximates this view"},
        "timing": timing,
        "exposure": exposure,
        "reference": scores,
        "sceneHash": view.scene_hash(),
        "shaderRevision": view.shader_revision(),
    });
    effects.destroy(gpu)?;
    view.destroy(gpu)?;
    Ok(report)
}

/// Diagnostic dumps (opt-in, `ARA_CAPTURE_SIGNALS`): accumulated direct light,
/// demodulated incident (indirect) light and filtered radiance, each exposed with
/// the view's current exposure and a simple Reinhard curve, at render size.
fn signals(
    gpu: &mut VulkanDevice,
    view: &FidelityView,
    options: &Options<'_>,
    label: &str,
    shot: &str,
) -> Result<()> {
    let exposure = view.capture_exposure(gpu)?.multiplier;
    let [w, h] = view.settings().render_size;
    for (name, data) in [
        ("direct", view.capture_direct_radiance(gpu)?),
        ("incident", view.capture_incident_radiance(gpu)?),
        ("radiance", view.capture_radiance(gpu)?),
    ] {
        // Incident light is demodulated irradiance-like; show it at the same exposure.
        let image = RgbaImage::from_fn(w, h, |x, y| {
            let p = data[(y * w + x) as usize];
            let c = [0, 1, 2].map(|i| {
                let v = (p[i] * exposure).max(0.);
                ((v / (1. + v)).powf(1. / 2.2) * 255.) as u8
            });
            image::Rgba([c[0], c[1], c[2], 255])
        });
        image.save(
            options
                .scratch
                .join(format!("{}-{label}-{shot}-{name}.png", options.name)),
        )?;
    }
    Ok(())
}

/// The Low preset: the baseline raster renderer on its own device, with the
/// environment's raster approximation (physical sky map, exposure, tone curve).
/// VFX are not composited into baseline captures.
fn baseline(
    mut scene: Scene,
    look: &Look,
    options: &Options<'_>,
    size: [u32; 2],
    truths: &[Truth],
) -> Result<Value> {
    let environment = options
        .environment
        .baseline(scene.sun_direction, scene.sun_lux);
    if let Some((sky_size, pixels)) = environment.sky {
        scene.environment_size = sky_size;
        scene.environment = pixels;
    }
    scene.sun_lux = environment.sun_lux;
    let created = std::time::Instant::now();
    let mut renderer = ara_render::Renderer::new(scene, size, &[], options.pipeline_cache)?;
    renderer.set_tone_curve(match environment.tone_mapper {
        ToneMapper::AcesFit => ToneCurve::AcesFit,
        ToneMapper::PbrNeutral => ToneCurve::Neutral,
        ToneMapper::Filmic => ToneCurve::Filmic,
    });
    let create_ms = created.elapsed().as_secs_f64() * 1000.0;
    let mut timing = Value::Null;
    let mut scores = Vec::new();
    for (i, shot) in shots(look, options).into_iter().enumerate() {
        let cam = camera(shot);
        if i == 0 {
            for _ in 0..options.settle {
                renderer.render(&cam, environment.exposure)?;
            }
            let before = gpu_load();
            let mut window = Window::default();
            for _ in 0..options.timing_frames {
                renderer.render(&cam, environment.exposure)?;
                window.push(renderer.timings());
            }
            timing = window.report(&format!("{} low", options.name), size, &shot.name, before);
        }
        let capture = renderer.capture(&CaptureRequest {
            camera: cam,
            size,
            mode: ViewMode::Lit,
            deterministic: false,
            exposure: environment.exposure,
            settle_frames: options.settle.min(1024),
            tick: 0,
            seed: 0,
            marks: None,
        })?;
        save(&capture.image, options, "low", &shot.name)?;
        if let Some(truth) = truths.iter().find(|t| t.shot == shot.name) {
            let scored = renderer.capture(&CaptureRequest {
                camera: camera(shot),
                size,
                mode: ViewMode::Lit,
                deterministic: false,
                exposure: ara_render_fidelity::settings::exposure_from_ev100(truth.ev100, 0.),
                settle_frames: options.settle.min(1024),
                tick: 0,
                seed: 0,
                marks: None,
            })?;
            let heatmap = options
                .scratch
                .join(format!("{}-low-{}-flip.png", options.name, shot.name));
            scores.push(score(&scored.image, truth, &heatmap)?);
        }
    }
    Ok(json!({
        "preset": "low",
        "renderer": "baseline",
        "renderSize": size,
        "displaySize": size,
        "exposure": environment.exposure,
        "toneMapper": format!("{:?}", environment.tone_mapper),
        "physicalSkyMap": options.environment.atmosphere.is_some_and(|a| a.physical_sky),
        "viewCreateMs": create_ms,
        "timing": timing,
        "reference": scores,
    }))
}

/// A camera move from one shot to another, captured into a horizontal strip.
#[allow(clippy::too_many_arguments)]
fn strip(
    gpu: &mut VulkanDevice,
    view: &mut FidelityView,
    effects: &mut ara_runtime_render::fidelity::vfx::Effects,
    look: &Look,
    sequence: &super::look::Sequence,
    tick: &mut u32,
    options: &Options<'_>,
    label: &str,
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
    // `ARA_CAPTURE_SEQUENCE_STEP` overrides the frames between strip images (pace).
    let step = std::env::var("ARA_CAPTURE_SEQUENCE_STEP")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(sequence.step);
    let steps = sequence.frames.saturating_sub(1).max(1) * step;
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
        if !effects.is_empty() {
            effects.composite(gpu, view, &cam, *tick)?;
        }
        *tick += 1;
        if s % step == 0 && frame_index < sequence.frames {
            let image = view.capture(gpu)?;
            if frame_index == sequence.frames / 2 {
                // A full-resolution mid-move frame shows motion grain at its real size.
                image.save(
                    options
                        .scratch
                        .join(format!("{}-{label}-motion.png", options.name)),
                )?;
            }
            let small = imageops::resize(&image, w, h, imageops::FilterType::Triangle);
            imageops::overlay(&mut out, &small, i64::from(frame_index * w), 0);
            frame_index += 1;
        }
    }
    out.save(
        options
            .evidence
            .join(format!("{}-{label}-sequence.png", options.name)),
    )?;
    Ok(())
}
