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
use ara_gpu::GpuDevice;
use ara_render::{
    capture::{CaptureRequest, ViewMode},
    scene::CameraSpec,
};
use ara_render_fidelity::{
    FidelityView, FrameInput, PostSettings,
    environment::FrameEnvironment,
    presets::{
        Capabilities, FidelityPlan, Plan, QualityChoice, QualityPreset, RasterPlan, RenderOptions,
        RenderPath,
    },
};
use ara_runtime::extract::{CameraView, RenderFrame};
use ara_runtime_render::{PresentationImage, Presenter, fidelity::native::Prepared as Scene};
use image::{RgbaImage, imageops};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

mod timing;

/// Display resolution of every capture.
pub const DISPLAY: [u32; 2] = [2560, 1440];

/// The capture display size: `DISPLAY`, or `ARA_CAPTURE_DISPLAY=<w>x<h>` (4K
/// captures match the cross-engine benchmark's region FLIP).
fn display() -> [u32; 2] {
    std::env::var("ARA_CAPTURE_DISPLAY")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some([w.trim().parse().ok()?, h.trim().parse().ok()?])
        })
        .unwrap_or(DISPLAY)
}
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
    /// Medium and above use the opt-in ray-traced renderer.
    pub ray_tracing: bool,
    /// Resolves the frame's native content (terrain materials, foliage) for the
    /// Low presenter, which bakes its probe GI from it.
    pub content: &'a dyn Fn(&RenderFrame) -> Result<ara_runtime_render::fidelity::native::Content>,
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
    fn push_with_composites(
        &mut self,
        timings: &[ara_gpu::vulkan::PassTiming],
        effects: f64,
        publish: f64,
    ) {
        self.push(timings);
        if let Some(total) = self.totals.last_mut() {
            *total += effects + publish;
        }
        self.sample("VFX.SimulationAndDraw", effects);
        self.sample("Presentation.Publish", publish);
    }
    fn sample(&mut self, name: &str, milliseconds: f64) {
        if let Some(samples) = self.passes.get_mut(name) {
            samples.push(milliseconds);
        } else {
            self.passes.insert(name.to_owned(), vec![milliseconds]);
        }
    }
    fn push(&mut self, timings: &[ara_gpu::vulkan::PassTiming]) {
        self.totals
            .push(timings.iter().map(|p| p.milliseconds).sum::<f64>());
        for p in timings {
            self.sample(&p.pass, p.milliseconds);
        }
    }
    /// Pass percentiles cover the frames that ran the pass (`frames`); cached
    /// passes skip frames, so `perFrame` spreads their total over the window.
    fn report(mut self, label: &str, size: [u32; 2], shot: &str, before: String) -> Value {
        let samples = self.totals.clone();
        self.totals.sort_by(f64::total_cmp);
        let window = self.totals.len().max(1) as f64;
        let passes: BTreeMap<String, Value> = self
            .passes
            .into_iter()
            .map(|(k, mut v)| {
                let per_frame = v.iter().sum::<f64>() / window;
                v.sort_by(f64::total_cmp);
                (
                    k,
                    json!({"p50": percentile(&v, 0.5), "p95": percentile(&v, 0.95),
                        "frames": v.len(), "perFrame": per_frame}),
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
    gpu: &mut GpuDevice,
    scene: Scene,
    frame: &mut RenderFrame,
    stats: &Stats,
    look: &Look,
    options: &Options<'_>,
) -> Result<Value> {
    let path = options
        .evidence
        .join(format!("{}-timings.json", options.name));
    timing::record(&path, || run_inner(gpu, scene, frame, stats, look, options))
}

fn run_inner(
    gpu: &mut GpuDevice,
    scene: Scene,
    frame: &mut RenderFrame,
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
    let mut raster = Vec::new();
    let render_options = RenderOptions {
        ray_tracing: options.ray_tracing,
    };
    for &preset in options.presets {
        let plan = Plan::resolve(
            QualityChoice::Preset(preset),
            &capabilities,
            display(),
            render_options,
        )?;
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
            RenderPath::Raster(plan) => raster.push((preset, plan)),
        }
    }
    if !raster.is_empty() {
        drop(scene);
        for (preset, plan) in raster {
            modes.push(baseline(frame, look, options, preset, &plan, &truths)?);
        }
    }
    let report = json!({
        "completed": true,
        "scene": look.scene,
        "name": options.name,
        "budgets": {"decision": "D26", "gpuMs": {"low": 2.0, "medium": 3.5, "high": 6.0, "epic": 10.0},
            "highCpuMs": 4.0, "postFirstFrameHitchMs": 16.0,
            "loadingPresentedMs": 2000, "fullSceneWarmMs": 3000},
        "content": stats,
        "referenceSettings": {"samples": options.reference_samples, "size": super::reference::SIZE,
            "renderer": "ara-pathtrace native reference transport", "vfx": false},
        "authoredSettingsOnly": std::env::var_os("ARA_CAPTURE_AUTHORED_ONLY").is_some(),
        "modes": modes,
        "sharedGpu": "other agents share this RTX 4070 SUPER; gpuBefore/gpuAfter record nvidia-smi load around each window",
    });
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

/// The raster capture's view: `ARA_CAPTURE_VIEW` = albedo, normals, cascades,
/// material (roughness/metal), wireframe or lit_wireframe for diagnosis; lit otherwise.
fn view_mode() -> ViewMode {
    match std::env::var("ARA_CAPTURE_VIEW").as_deref() {
        Ok("albedo") => ViewMode::Albedo,
        Ok("normals") => ViewMode::Normals,
        Ok("cascades") => ViewMode::ShadowCascades,
        Ok("material") => ViewMode::RoughnessMetal,
        Ok("wireframe") => ViewMode::Wireframe,
        Ok("lit_wireframe") => ViewMode::LitWireframe,
        _ => ViewMode::Lit,
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
    gpu: &mut GpuDevice,
    scene: &Scene,
    look: &Look,
    options: &Options<'_>,
    label: &str,
    plan: &FidelityPlan,
    truths: &[Truth],
) -> Result<Value> {
    let memory_before = gpu_memory();
    let created = std::time::Instant::now();
    let mut view = super::convert::view(
        gpu,
        scene,
        plan.settings(options.environment.post(PostSettings::default())),
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
            scene.scene.sun_direction,
            options
                .environment
                .ground_sun(scene.scene.sun_direction, scene.scene.sun_lux),
        ),
    )?;
    let create_ms = created.elapsed().as_secs_f64() * 1000.0;
    let mut tick = 0;
    let mut timing = Value::Null;
    let mut scores = Vec::new();
    let mut published = None;
    for (i, shot) in shots(look, options).into_iter().enumerate() {
        let cam = camera(shot);
        for n in 0..options.settle {
            view.render(gpu, &frame(&cam, tick, n == 0))?;
            if !effects.is_empty() {
                effects.composite(gpu, &mut view, &cam, tick)?;
            }
            tick += 1;
        }
        if i == 0 {
            PresentationImage::publish(&mut published, gpu, view.output(), plan.display_size)?;
            let mut guard = ara_bench::passes::Guard::configured(
                ara_bench::passes::Expected::Fidelity,
                std::time::Duration::from_secs(120),
            )?;
            let ready_started = std::time::Instant::now();
            loop {
                view.render(gpu, &frame(&cam, tick, false))?;
                if !effects.is_empty() {
                    effects.composite(gpu, &mut view, &cam, tick)?;
                }
                tick += 1;
                let passes = ara_bench::passes::Frame::from_names(
                    view.timings().iter().map(|pass| pass.pass.as_str()),
                );
                if guard.wait(ready_started.elapsed(), Some(passes))? {
                    break;
                }
            }
            let before = gpu_load();
            let mut window = Window::default();
            for _ in 0..options.timing_frames {
                view.render(gpu, &frame(&cam, tick, false))?;
                if !effects.is_empty() {
                    effects.composite(gpu, &mut view, &cam, tick)?;
                }
                tick += 1;
                let publish = PresentationImage::publish(
                    &mut published,
                    gpu,
                    view.output(),
                    plan.display_size,
                )?;
                guard.record(ara_bench::passes::Frame::from_names(
                    view.timings().iter().map(|pass| pass.pass.as_str()),
                ))?;
                window.push_with_composites(view.timings(), effects.gpu_ms(), publish);
            }
            timing = window.report(
                &format!("{} {label}", options.name),
                render,
                &shot.name,
                before,
            );
            timing["pass_presence"] = guard.report();
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
            // VFX are absent from path-traced truth. Clear their accumulated
            // contribution, then reconverge DDGI and AA at the fixed exposure.
            view.reset_accumulation(gpu)?;
            for _ in 0..options.settle.saturating_add(SCORE_FRAMES) {
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
        "timingScope": "transport, post, VFX and publication; static poses and settled texture residency",
        "exposure": exposure,
        "reference": scores,
        "sceneHash": view.scene_hash(),
        "shaderRevision": view.shader_revision(),
    });
    if let Some(published) = published {
        published.destroy(gpu)?;
    }
    effects.destroy(gpu)?;
    view.destroy(gpu)?;
    Ok(report)
}

/// Diagnostic dumps (opt-in, `ARA_CAPTURE_SIGNALS`): accumulated direct light,
/// demodulated incident (indirect) light and filtered radiance, each exposed with
/// the view's current exposure and a simple Reinhard curve, at render size.
fn signals(
    gpu: &mut GpuDevice,
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
        if name != "radiance" {
            // Temporal sample count (alpha) as grey: white is 24 or more frames.
            let count = RgbaImage::from_fn(w, h, |x, y| {
                let v = (data[(y * w + x) as usize][3] / 24.).clamp(0., 1.);
                let c = (v * 255.) as u8;
                image::Rgba([c, c, c, 255])
            });
            count.save(
                options
                    .scratch
                    .join(format!("{}-{label}-{shot}-{name}-count.png", options.name)),
            )?;
        }
    }
    Ok(())
}

/// Low uses the actual shipping raster presenter, including terrain and VFX.
/// Its timing scope names the base raster passes until all composites expose timestamps.
fn baseline(
    frame: &mut RenderFrame,
    look: &Look,
    options: &Options<'_>,
    preset: QualityPreset,
    plan: &RasterPlan,
    truths: &[Truth],
) -> Result<Value> {
    // The presenter, its captures and timings are at the display size; the scene
    // renders at the plan's render size (temporal upscaling).
    let size = plan.display_size;
    let name = preset.name();
    let created = std::time::Instant::now();
    let mut presenter = Presenter::new(size, Vec::new(), options.pipeline_cache);
    presenter.set_render_options(RenderOptions::default());
    // `ARA_RENDER_SCALE=<0..1>` overrides the preset's raster render scale (timing and
    // capture A/Bs of upscaler modes).
    presenter.set_render_scale(
        std::env::var("ARA_RENDER_SCALE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok()),
    );
    presenter.set_quality(QualityChoice::Preset(preset));
    // The same resolved look (scene components plus look-file overrides) as the
    // fidelity modes and references.
    presenter.set_look_override(Some(*options.environment));
    presenter.set_probe_gi(true);
    presenter.prepare_with(frame, || (options.content)(frame))?;
    // `ARA_CAPTURE_COLD=1` starts the first shot as Play does, without the probe GI
    // wait, settle or timing, so `ARA_CAPTURE_LIVE` shows a fresh player's frames.
    let cold = std::env::var_os("ARA_CAPTURE_COLD").is_some();
    if !cold {
        presenter.wait_probe_gi()?;
        // Timings and shots need the full path: hybrid ray-query and live-effect PSOs
        // compile off-thread while frames present (bounded, as the capture's own wait).
        let started = std::time::Instant::now();
        while presenter.readiness().pipelines_pending && started.elapsed().as_secs() < 120 {
            presenter.present(frame)?;
        }
    }
    let create_ms = created.elapsed().as_secs_f64() * 1000.;
    let mut timing = Value::Null;
    let mut scores = Vec::new();
    for (i, shot) in shots(look, options).into_iter().enumerate() {
        frame.camera = Some(CameraView {
            position: shot.position,
            forward: (glam::Vec3::from_array(shot.target) - glam::Vec3::from_array(shot.position))
                .normalize()
                .to_array(),
            fov: shot.fov,
        });
        let first_cold = cold && i == 0;
        for _ in 0..if first_cold { 0 } else { options.settle } {
            presenter.present(frame)?;
        }
        if i == 0 && !cold {
            let hardware_rt = presenter
                .renderer()
                .context("capture presenter")?
                .gpu()
                .info()
                .ray_tracing;
            let mut guard = ara_bench::passes::Guard::configured(
                ara_bench::passes::Expected::resolve(name, hardware_rt, options.ray_tracing),
                std::time::Duration::from_secs(120),
            )?;
            let ready_started = std::time::Instant::now();
            loop {
                presenter.present(frame)?;
                if guard.wait(ready_started.elapsed(), timing::frame(&presenter))? {
                    break;
                }
            }
            let before = gpu_load();
            let mut window = Window::default();
            // `ARA_CAPTURE_MOTION=1` walks the camera forward (1.5 m/s) while it
            // turns (20 degrees per second), so cached passes are timed in motion.
            let motion = std::env::var_os("ARA_CAPTURE_MOTION").is_some();
            for k in 0..options.timing_frames {
                if motion {
                    frame.camera = Some(moving(shot, k as f32 / 60.));
                }
                presenter.present(frame)?;
                guard.record(timing::frame(&presenter).unwrap_or_default())?;
                window.push(presenter.renderer().context("Low presenter")?.timings());
            }
            let mode = if motion { " (moving)" } else { "" };
            timing = window.report(
                &format!("{} {name} raster{mode}", options.name),
                size,
                &shot.name,
                before,
            );
            timing["pass_presence"] = guard.report();
        }
        // `ARA_CAPTURE_STUDY` captures the upscaler study's frames (capture/study.rs).
        super::study::run(&mut presenter, frame, shot, options, size)?;
        // `ARA_CAPTURE_LIVE=N` saves every fourth of N live frames as Play shows
        // them (TAA, jitter and half-resolution history running, no capture
        // reset); with `ARA_CAPTURE_MOTION` the camera walks while it turns.
        if let Some(frames) = std::env::var("ARA_CAPTURE_LIVE")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        {
            let motion = std::env::var_os("ARA_CAPTURE_MOTION").is_some();
            // `ARA_CAPTURE_DOLLY=<m/s>` moves the camera straight along its view.
            let dolly = std::env::var("ARA_CAPTURE_DOLLY")
                .ok()
                .and_then(|v| v.parse::<f32>().ok());
            for k in 0..frames {
                if motion {
                    frame.camera = Some(moving(shot, k as f32 / 60.));
                }
                if let Some(speed) = dolly {
                    let from = glam::Vec3::from_array(shot.position);
                    let forward = (glam::Vec3::from_array(shot.target) - from).normalize();
                    frame.camera = Some(CameraView {
                        position: (from + forward * speed * k as f32 / 60.).to_array(),
                        forward: forward.to_array(),
                        fov: shot.fov,
                    });
                }
                presenter.present(frame)?;
                if k % 4 == 3 {
                    let request = CaptureRequest {
                        camera: camera(shot),
                        size,
                        mode: ViewMode::Lit,
                        deterministic: false,
                        exposure: frame.environment.exposure,
                        settle_frames: 1,
                        tick: frame.tick,
                        seed: 0,
                        marks: None,
                    };
                    let renderer = presenter.renderer_mut().context("live raster renderer")?;
                    let live = renderer.capture_completed(&request)?;
                    println!(
                        "live {k:03}: exposure {:?}, readiness {}",
                        renderer.exposure()?,
                        serde_json::to_string(&presenter.readiness())?
                    );
                    live.image.save(options.scratch.join(format!(
                        "{}-{name}-{}-live{k:03}.png",
                        options.name, shot.name
                    )))?;
                }
            }
        }
        let request = CaptureRequest {
            camera: camera(shot),
            size,
            mode: view_mode(),
            deterministic: false,
            exposure: frame.environment.exposure,
            settle_frames: options.settle.min(1024),
            tick: frame.tick,
            seed: 0,
            marks: None,
        };
        let capture = presenter.capture_with(frame, &request, || Ok(Default::default()))?;
        save(&capture.image, options, name, &shot.name)?;
        if let Some(truth) = truths.iter().find(|t| t.shot == shot.name) {
            // Score at the reference's exposure, without compensation.
            presenter.set_look_override(Some(FrameEnvironment {
                ev100: Some(truth.ev100),
                compensation: 0.,
                ..*options.environment
            }));
            let exposure = frame.environment.exposure;
            frame.environment.exposure =
                ara_render_fidelity::settings::exposure_from_ev100(truth.ev100, 0.);
            let effects = std::mem::take(&mut frame.effects);
            let scored = presenter.capture_with(frame, &request, || Ok(Default::default()));
            frame.effects = effects;
            presenter.set_look_override(Some(*options.environment));
            frame.environment.exposure = exposure;
            let scored = scored?;
            if std::env::var_os("ARA_CAPTURE_SCORED").is_some() {
                // Opt-in: the frame FLIP scores, at the reference's exposure.
                scored.image.save(
                    options
                        .scratch
                        .join(format!("{}-{name}-{}-scored.png", options.name, shot.name)),
                )?;
            }
            let heatmap = options
                .scratch
                .join(format!("{}-{name}-{}-flip.png", options.name, shot.name));
            scores.push(score(&scored.image, truth, &heatmap)?);
        }
    }
    Ok(
        json!({"preset":name,"renderer":"raster shipping presenter", "renderSize":plan.render_size,"displaySize":size,
        "viewCreateMs":create_ms,"timing":timing,"timingScope":"base raster passes; terrain/VFX/composite timestamps pending",
        "reference":scores}),
    )
}
/// `shot`'s camera `seconds` into a walk: 1.5 m/s along its level heading while
/// turning 20 degrees per second.
fn moving(shot: &Shot, seconds: f32) -> CameraView {
    let position = glam::Vec3::from_array(shot.position);
    let forward = (glam::Vec3::from_array(shot.target) - position).normalize();
    let level = glam::Vec3::new(forward.x, 0., forward.z).normalize_or_zero();
    let turn = glam::Quat::from_rotation_y((20. * seconds).to_radians());
    CameraView {
        position: (position + level * 1.5 * seconds).to_array(),
        forward: (turn * forward).to_array(),
        fov: shot.fov,
    }
}

/// A camera move from one shot to another, captured into a horizontal strip.
#[allow(clippy::too_many_arguments)]
fn strip(
    gpu: &mut GpuDevice,
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
                if std::env::var_os("ARA_CAPTURE_SIGNALS").is_some() {
                    signals(gpu, view, options, label, "motion")?;
                }
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
