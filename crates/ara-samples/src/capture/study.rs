//! Upscaler studies (`ARA_CAPTURE_STUDY`) on the shipping raster presenter: live
//! frames of one shot at several render scales, a static window then a slow pan,
//! and (`ARA_CAPTURE_STUDY=gt`) the supersampled ground truth of the same poses.
//!
//! - `ARA_CAPTURE_STUDY=0.5,0.667,1`: render scales to capture with the
//!   renderer's upscaler (`ARA_UPSCALER` selects it at creation);
//!   `ARA_CAPTURE_STUDY=gt`: native resolution, a 4×4 grid of fixed sub-pixel
//!   offsets per pose (deterministic captures, no history), averaged in linear light.
//! - `ARA_STUDY_LABEL` names the run's directory (default `run`).
//! - `ARA_STUDY_PAN` is the pan rate in degrees of yaw per second (default 1.5:
//!   about 1.4 display pixels a frame at 1440p and 46°).
//!
//! Frames land in `<scratch>/study/<shot>/<label>/<scale>/`: `static-NN.png`
//! (16 consecutive frames after a 96-frame settle), then `pan-NNN.png` pairs of
//! consecutive frames from the pan that follows without a history reset. The
//! ground truth writes `gt/static.png` and `gt/pan-NNN.png` for the same poses.
//! Exposure is manual for every run: `exposure.json` beside the runs records the
//! EV100 the first run metered (or the look's manual EV100), and later runs reuse it.
use super::{
    look::Shot,
    render::{Options, camera},
};
use anyhow::{Context, Result};
use ara_render::capture::{CaptureRequest, ViewMode};
use ara_render_fidelity::{environment::FrameEnvironment, settings::exposure_from_ev100};
use ara_runtime::extract::{CameraView, RenderFrame};
use ara_runtime_render::Presenter;
use image::RgbaImage;
use std::path::Path;

/// Frames rendered at the shot before the static window.
const SETTLE: u32 = 96;
/// Consecutive static frames saved.
const STATIC_FRAMES: u32 = 16;
/// Pan frames; the pair (k, k + 1) is saved for every k = 15 (mod 16).
const PAN_FRAMES: u32 = 96;
/// Frames per ground-truth sub-sample capture (SSR and history-fed passes settle).
const GT_SETTLE: u32 = 6;
/// Ground-truth sub-sample grid per axis.
const GT_GRID: u32 = 4;

/// The pan's saved frame indices, in order.
fn pan_saves() -> impl Iterator<Item = u32> {
    (1..PAN_FRAMES).filter(|k| k % 16 == 15 || k % 16 == 0)
}

/// `shot`'s camera `frames` into the pan: yaw about the eye at `rate` degrees per
/// second (60 frames per second), pitch unchanged.
fn pan(shot: &Shot, frames: u32, rate: f32) -> CameraView {
    let position = glam::Vec3::from_array(shot.position);
    let forward = (glam::Vec3::from_array(shot.target) - position).normalize();
    let yaw = (rate * frames as f32 / 60.).to_radians();
    CameraView {
        position: shot.position,
        forward: (glam::Quat::from_rotation_y(yaw) * forward).to_array(),
        fov: shot.fov,
    }
}

/// The capture camera for a pan pose (the look-at form `camera` builds for a shot).
fn pose_shot(shot: &Shot, view: &CameraView) -> Shot {
    let position = glam::Vec3::from_array(view.position);
    Shot {
        name: shot.name.clone(),
        position: view.position,
        target: (position + glam::Vec3::from_array(view.forward) * 10.).to_array(),
        fov: view.fov,
    }
}

/// Runs the study configured by `ARA_CAPTURE_STUDY` for `shot` on `presenter`.
pub(super) fn run(
    presenter: &mut Presenter,
    frame: &mut RenderFrame,
    shot: &Shot,
    options: &Options<'_>,
    size: [u32; 2],
) -> Result<()> {
    let Ok(spec) = std::env::var("ARA_CAPTURE_STUDY") else {
        return Ok(());
    };
    let label = std::env::var("ARA_STUDY_LABEL").unwrap_or_else(|_| "run".into());
    let rate = std::env::var("ARA_STUDY_PAN")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(1.5);
    let root = options.scratch.join("study").join(&shot.name);
    std::fs::create_dir_all(&root)?;
    frame.camera = Some(pan(shot, 0, rate));
    let ev100 = lock_exposure(presenter, frame, &root, options.environment)?;
    println!("study {}: EV100 {ev100:.3}, pan {rate} deg/s", shot.name);
    if spec.trim() == "gt" {
        truth(presenter, frame, shot, size, rate, &root.join("gt"))?;
    } else {
        for scale in spec.split(',') {
            let value: f32 = scale.trim().parse().context("ARA_CAPTURE_STUDY scale")?;
            let dir = root.join(&label).join(scale.trim());
            live(presenter, frame, shot, size, rate, value, &dir)?;
        }
    }
    presenter.set_render_scale(None);
    presenter.set_look_override(Some(*options.environment));
    Ok(())
}

/// Fixes a manual exposure shared by every run of this shot (`exposure.json`).
fn lock_exposure(
    presenter: &mut Presenter,
    frame: &mut RenderFrame,
    root: &Path,
    environment: &FrameEnvironment,
) -> Result<f32> {
    let path = root.join("exposure.json");
    let recorded = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v["ev100"].as_f64())
        .map(|v| v as f32);
    let ev100 = if let Some(ev) = recorded.or(environment.ev100) {
        ev
    } else {
        // Meter at the shot with the authored automatic exposure.
        presenter.set_look_override(Some(*environment));
        for _ in 0..SETTLE {
            presenter.present(frame)?;
        }
        let multiplier = presenter
            .renderer()
            .context("study renderer")?
            .exposure()?
            .context("metered exposure")?;
        -(1.2 * multiplier).log2()
    };
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"ev100": ev100}))?,
    )?;
    presenter.set_look_override(Some(FrameEnvironment {
        ev100: Some(ev100),
        compensation: 0.,
        ..*environment
    }));
    frame.environment.exposure = exposure_from_ev100(ev100, 0.);
    Ok(ev100)
}

/// Live frames at render `scale`: the static window, then the pan.
fn live(
    presenter: &mut Presenter,
    frame: &mut RenderFrame,
    shot: &Shot,
    size: [u32; 2],
    rate: f32,
    scale: f32,
    dir: &Path,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    presenter.set_render_scale(Some(scale));
    frame.camera = Some(pan(shot, 0, rate));
    for _ in 0..SETTLE {
        presenter.present(frame)?;
    }
    for k in 0..STATIC_FRAMES {
        presenter.present(frame)?;
        completed(presenter, frame, shot, size)?.save(dir.join(format!("static-{k:02}.png")))?;
    }
    let saves: Vec<u32> = pan_saves().collect();
    for k in 0..PAN_FRAMES {
        frame.camera = Some(pan(shot, k, rate));
        presenter.present(frame)?;
        if saves.contains(&k) {
            let pose = pose_shot(shot, frame.camera.as_ref().context("pan camera")?);
            completed(presenter, frame, &pose, size)?.save(dir.join(format!("pan-{k:03}.png")))?;
        }
    }
    println!(
        "study {} scale {scale}: {} frames saved",
        shot.name,
        STATIC_FRAMES as usize + saves.len()
    );
    Ok(())
}

/// Reads back the frame just presented (live: jitter and history running).
fn completed(
    presenter: &mut Presenter,
    frame: &RenderFrame,
    shot: &Shot,
    size: [u32; 2],
) -> Result<RgbaImage> {
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
    let renderer = presenter.renderer_mut().context("study renderer")?;
    Ok(renderer.capture_completed(&request)?.image)
}

/// Supersampled ground truth of the static pose and every saved pan pose.
fn truth(
    presenter: &mut Presenter,
    frame: &mut RenderFrame,
    shot: &Shot,
    size: [u32; 2],
    rate: f32,
    dir: &Path,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    presenter.set_render_scale(Some(1.));
    let mut poses = vec![(0, "static.png".to_owned())];
    poses.extend(pan_saves().map(|k| (k, format!("pan-{k:03}.png"))));
    for (k, file) in poses {
        frame.camera = Some(pan(shot, k, rate));
        let pose = pose_shot(shot, frame.camera.as_ref().context("pan camera")?);
        let mut sum = vec![0f32; (size[0] * size[1] * 3) as usize];
        for j in 0..GT_GRID {
            for i in 0..GT_GRID {
                let offset = |n: u32| (n as f32 + 0.5) / GT_GRID as f32 - 0.5;
                presenter
                    .renderer_mut()
                    .context("study renderer")?
                    .set_jitter_override(Some([offset(i), offset(j)]));
                let request = CaptureRequest {
                    camera: camera(&pose),
                    size,
                    mode: ViewMode::Lit,
                    deterministic: true,
                    exposure: frame.environment.exposure,
                    settle_frames: GT_SETTLE,
                    tick: frame.tick,
                    seed: 0,
                    marks: None,
                };
                let capture = presenter.capture_with(frame, &request, || Ok(Default::default()));
                presenter
                    .renderer_mut()
                    .context("study renderer")?
                    .set_jitter_override(None);
                let image = capture?.image;
                for (s, p) in sum.chunks_exact_mut(3).zip(image.pixels()) {
                    for c in 0..3 {
                        s[c] += decode(p.0[c]);
                    }
                }
            }
        }
        let n = (GT_GRID * GT_GRID) as f32;
        let mut out = RgbaImage::new(size[0], size[1]);
        for (p, s) in out.pixels_mut().zip(sum.chunks_exact(3)) {
            p.0 = [encode(s[0] / n), encode(s[1] / n), encode(s[2] / n), 255];
        }
        out.save(dir.join(&file))?;
        println!("study {} ground truth {file}", shot.name);
    }
    Ok(())
}

/// sRGB byte to linear light.
fn decode(v: u8) -> f32 {
    let x = v as f32 / 255.;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light to an sRGB byte.
fn encode(x: f32) -> u8 {
    let x = x.clamp(0., 1.);
    let s = if x <= 0.0031308 {
        x * 12.92
    } else {
        1.055 * x.powf(1. / 2.4) - 0.055
    };
    (s * 255. + 0.5) as u8
}
