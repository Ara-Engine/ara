//! `--verify-scene-quality`: what the Scene view shows over time, captured as the user
//! sees it (the presented frame's Scene region), next to what the renderer was doing on
//! each frame. From open, a capture every [`OPEN_EVERY`] frames for [`OPEN_SECONDS`],
//! then a settled reference; then an orbit, a fly-forward and a pan, each followed by a
//! hold (captured closely) and a settled reference. Offscreen only. Writes
//! `scene-quality.json` (one row per frame: render and display size, preset, texture
//! residency, whether the Scene still wants frames and why, the last completed Scene
//! frame's GPU start and pass time, camera, imports) and the
//! PNGs; offline tooling scores each capture against its phase's settled reference.
use std::path::PathBuf;
use std::time::Instant;

use ara_editor_gpu::NativeUi;
use ara_render::Renderer;
use ara_render_fidelity::presets::{Capabilities, Plan, RenderOptions, RenderPath};
use glam::Vec3;
use serde_json::{Value, json};

use super::viewport::SceneView;
use crate::{Editor, shell::Shell};

/// How long the open phase captures, and how often.
const OPEN_SECONDS: f64 = 10.;
const OPEN_EVERY: u32 = 6;
/// Frames each camera move lasts, and the hold after it.
const MOVE_FRAMES: u32 = 90;
const HOLD_FRAMES: u32 = 150;
/// A settled reference: the Scene had no content work (snapshots, publications, GI,
/// fades) for this many frames in a row, rendering temporal frames meanwhile: three
/// times what its accumulation takes to converge (27-36 frames)...
const SETTLED_FRAMES: u32 = 90;
/// ...or this long passed (live content never settles).
const SETTLE_CAP_SECONDS: f64 = 20.;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Move {
    Orbit,
    Fly,
    Pan,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Open,
    Settle(&'static str),
    Move(Move),
    Hold(Move),
}
const PHASES: &[Phase] = &[
    Phase::Open,
    Phase::Settle("open"),
    Phase::Move(Move::Orbit),
    Phase::Hold(Move::Orbit),
    Phase::Settle("orbit"),
    Phase::Move(Move::Fly),
    Phase::Hold(Move::Fly),
    Phase::Settle("fly"),
    Phase::Move(Move::Pan),
    Phase::Hold(Move::Pan),
    Phase::Settle("pan"),
];

/// The route's state; [`SceneQuality::step`] before each frame, [`SceneQuality::end`]
/// after it is presented.
pub(super) struct SceneQuality {
    output: PathBuf,
    phase: usize,
    age: u32,
    phase_started: Instant,
    started: Instant,
    quiet: u32,
    capture: Option<String>,
    rows: Vec<Value>,
}

impl SceneQuality {
    pub fn new(output: &std::path::Path) -> Self {
        Self {
            output: output.to_owned(),
            phase: 0,
            age: 0,
            phase_started: Instant::now(),
            started: Instant::now(),
            quiet: 0,
            capture: None,
            rows: Vec::new(),
        }
    }
    pub fn finished(&self) -> bool {
        self.phase >= PHASES.len()
    }
    fn label(phase: Phase) -> String {
        match phase {
            Phase::Open => "open".into(),
            Phase::Settle(name) => format!("settled-{name}"),
            Phase::Move(m) => format!("{m:?}").to_lowercase(),
            Phase::Hold(m) => format!("hold-{}", format!("{m:?}").to_lowercase()),
        }
    }
    fn advance(&mut self) {
        self.phase += 1;
        self.age = 0;
        self.quiet = 0;
        self.phase_started = Instant::now();
    }

    /// Before the frame: move the camera and decide whether this frame is captured.
    /// True when the camera changed (the Scene must render).
    pub fn step(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(&phase) = PHASES.get(self.phase) else {
            return Ok(false);
        };
        self.age += 1;
        let file = |s: &Self| format!("{}-{:04}.png", Self::label(phase), s.age);
        self.capture = match phase {
            Phase::Open => self.age.is_multiple_of(OPEN_EVERY).then(|| file(self)),
            Phase::Move(_) => self.age.is_multiple_of(3).then(|| file(self)),
            Phase::Hold(_) => (self.age <= 60 && self.age.is_multiple_of(3)
                || self.age.is_multiple_of(10))
            .then(|| file(self)),
            Phase::Settle(_) => None,
        };
        let Phase::Move(kind) = phase else {
            return Ok(false);
        };
        let mut camera = editor.state.camera;
        // Camera basis from the orbit angles: forward points from the eye to the target.
        let forward = Vec3::new(
            -camera.yaw.sin() * camera.pitch.cos(),
            -camera.pitch.sin(),
            -camera.yaw.cos() * camera.pitch.cos(),
        )
        .normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        match kind {
            Move::Orbit => camera.yaw += 0.02,
            Move::Fly => camera.target += forward * (camera.distance * 0.006),
            Move::Pan => camera.target += right * (camera.distance * 0.006),
        }
        editor.action(
            "editor.camera",
            json!({"target": camera.target.to_array(), "yaw": camera.yaw, "pitch": camera.pitch, "distance": camera.distance}),
        )?;
        Ok(true)
    }

    /// After the frame: record what the renderer was doing, capture the presented Scene
    /// region if this frame was chosen, and move to the next phase when this one ends.
    #[allow(
        clippy::too_many_arguments,
        reason = "the loop's GPU owners are borrowed separately"
    )]
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        renderer: &Renderer,
        scene: &SceneView,
        scene_ms: f64,
        presented: bool,
    ) -> ara_ui::Result<()> {
        let Some(&phase) = PHASES.get(self.phase) else {
            return Ok(());
        };
        let display = renderer.size();
        let settings = crate::world::gltf::render_settings(editor.host.store().workspace())?;
        let choice = editor
            .state
            .render_quality
            .unwrap_or(settings.quality.into());
        let plan = Plan::resolve(
            choice,
            &Capabilities::of(renderer.gpu()),
            display,
            RenderOptions {
                ray_tracing: settings.ray_tracing,
            },
        )?;
        let render = match plan.path {
            RenderPath::Raster(raster) => raster.render_size,
            RenderPath::Fidelity(_) => display,
        };
        let textures = renderer.native_texture_stats();
        // The last completed Scene frame's passes: a new start time is a new frame.
        let timings = renderer.timings();
        let gpu_start = timings.iter().map(|t| t.start_ms).reduce(f64::min);
        let gpu_ms: f64 = timings.iter().map(|t| t.milliseconds).sum();
        let wants = scene.needs_frame(editor.state.quality);
        let shared = scene.preview_needs_frame();
        let importing = shell
            .import_counts()
            .is_none_or(|(pending, _, _)| pending > 0);
        let launched = super::LAUNCHED
            .get()
            .map_or(0., |t| t.elapsed().as_secs_f64() * 1000.);
        let mut capture = None;
        if let Some(name) = self.capture.take()
            && presented
            && self.save_scene(shell, gpu, &name)?
        {
            capture = Some(name);
        }
        let camera = editor.state.camera;
        self.rows.push(json!({
            "tMs": launched,
            "phase": Self::label(phase),
            "age": self.age,
            "display": display,
            "render": render,
            "preset": format!("{:?}", plan.preset),
            "requested": format!("{:?}", plan.requested),
            "rasterView": editor.state.quality,
            "pendingTextures": textures.map(|t| t.pending_textures),
            "residentBytes": textures.map(|t| t.resident_bytes),
            "requestedBytes": textures.map(|t| t.requested_bytes),
            "uploadedBytes": textures.map(|t| t.uploaded_bytes),
            "sceneWantsFrames": wants,
            "sharedWork": shared,
            "sceneRendered": scene_ms > 0.,
            "sceneMs": scene_ms,
            "gpuStartMs": gpu_start,
            "gpuMs": gpu_ms,
            "importing": importing,
            "camera": {"target": camera.target.to_array(), "yaw": camera.yaw, "pitch": camera.pitch, "distance": camera.distance},
            "capture": capture,
        }));
        let done = match phase {
            Phase::Open => self.started.elapsed().as_secs_f64() >= OPEN_SECONDS,
            Phase::Move(_) => self.age >= MOVE_FRAMES,
            Phase::Hold(_) => self.age >= HOLD_FRAMES,
            Phase::Settle(name) => {
                // Content work, not `wants`: a native Scene keeps submitting frames
                // while it is shown, so it never stops wanting them.
                self.quiet = if shared || importing {
                    0
                } else {
                    self.quiet + 1
                };
                let settled = self.quiet >= SETTLED_FRAMES
                    || self.phase_started.elapsed().as_secs_f64() >= SETTLE_CAP_SECONDS;
                if settled && presented {
                    // The settled reference, from the same presented frame as the series.
                    let file = format!("settled-{name}.png");
                    self.save_scene(shell, gpu, &file)?;
                    if let Some(row) = self.rows.last_mut() {
                        row["capture"] = json!(format!("settled-{name}.png"));
                        row["settledAfterQuietFrames"] = json!(self.quiet);
                    }
                }
                settled && presented
            }
        };
        if done {
            self.advance();
        }
        Ok(())
    }

    /// Save the presented frame's Scene region (physical pixels) as `name`; false when
    /// the Scene view is not on screen.
    fn save_scene(&self, shell: &Shell, gpu: &mut NativeUi, name: &str) -> ara_ui::Result<bool> {
        let Some(rect) = shell.viewport() else {
            return Ok(false);
        };
        let dpi = gpu.dpi();
        let pixels = gpu.pixels()?;
        let x = (rect.x * dpi).round().max(0.) as u32;
        let y = (rect.y * dpi).round().max(0.) as u32;
        let w = ((rect.w * dpi).round() as u32).min(pixels.width().saturating_sub(x));
        let h = ((rect.h * dpi).round() as u32).min(pixels.height().saturating_sub(y));
        if w == 0 || h == 0 {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.output)?;
        image::imageops::crop_imm(&pixels, x, y, w, h)
            .to_image()
            .save(self.output.join(name))?;
        Ok(true)
    }

    /// Write `scene-quality.json`.
    pub fn write(&self, device: &Value) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("scene-quality.json"),
            ara_json::to_canonical_string(&json!({
                "device": device,
                "openSeconds": OPEN_SECONDS,
                "frames": self.rows,
            })),
        )?;
        Ok(())
    }
}
