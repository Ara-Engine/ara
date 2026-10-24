//! Ground truth for captures: each shot path-traced by the fidelity view's
//! undenoised reference transport (the ara-pathtrace contract: same extraction,
//! materials, lights and environment) at 1280×720, then every preset scored against
//! it without VFX. The primary transport error is linear HDR radiance RMSE (absolute
//! and relative to reference energy), with the preset's radiance area-resampled to
//! the reference size; display errors at one shared fixed exposure follow: sRGB RMSE
//! and LDR ꟻLIP at 67 pixels per degree.
use super::{look::Shot, render::Options};
use anyhow::Result;
use ara_gpu::vulkan::VulkanDevice;
use ara_render::scene::Scene;
use ara_render_fidelity::{
    FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler,
    environment::ev100_from_exposure,
};
use image::{RgbaImage, imageops};
use serde_json::{Value, json};
use std::path::Path;

/// Resolution of references and of the scored (downsampled) captures.
pub const SIZE: [u32; 2] = [1280, 720];
/// Maximum scattering events of the reference paths.
const BOUNCES: u32 = 8;

/// One converged reference image and the exposure every comparison uses.
pub struct Truth {
    /// Shot name.
    pub shot: String,
    /// Display-referred reference at [`SIZE`].
    pub image: RgbaImage,
    /// EV100 (no compensation) the reference converged to; comparisons reuse it.
    pub ev100: f32,
    /// Accumulated linear radiance at [`SIZE`], before exposure and post.
    pub radiance: Vec<[f32; 4]>,
}

/// Path-traces `shots` with `samples` samples per pixel, one view at a time.
pub fn render(
    gpu: &mut VulkanDevice,
    scene: &Scene,
    shots: &[&Shot],
    samples: u32,
    options: &Options<'_>,
) -> Result<Vec<Truth>> {
    let (environment, scratch, name) = (options.environment, options.scratch, options.name);
    let mut truths = Vec::new();
    for shot in shots {
        let started = std::time::Instant::now();
        let mut view = FidelityView::new(
            gpu,
            scene,
            Settings {
                render_size: SIZE,
                display_size: SIZE,
                tier: Tier::Rt,
                mode: Mode::Reference {
                    samples,
                    bounces: BOUNCES,
                },
                upscaler: Upscaler::Taau,
                probe_ray_budget: 64,
                reflection_cutoff: 1.,
                post: environment.post(PostSettings::default()),
            },
            None,
        )?;
        view.set_environment(environment)?;
        let cam = super::render::camera(shot);
        let input = |reset| FrameInput {
            camera: &cam,
            tick: 0,
            delta_seconds: 1. / 60.,
            reset,
            lighting_changed: false,
            deterministic: true,
        };
        view.render(gpu, &input(true))?;
        while view.reference_samples() < samples {
            view.render(gpu, &input(false))?;
        }
        let ev100 = ev100_from_exposure(view.capture_exposure(gpu)?.multiplier);
        let radiance = view.capture_radiance(gpu)?;
        view.set_post(PostSettings {
            ev100: Some(ev100),
            compensation: 0.,
            ..view.settings().post
        })?;
        view.render(gpu, &input(false))?;
        let image = view.capture(gpu)?;
        image.save(scratch.join(format!("{name}-reference-{}.png", shot.name)))?;
        eprintln!(
            "reference {} {}: {samples} spp in {:.1} s, EV100 {ev100:.2}",
            name,
            shot.name,
            started.elapsed().as_secs_f64()
        );
        view.destroy(gpu)?;
        truths.push(Truth {
            shot: shot.name.clone(),
            image,
            ev100,
            radiance,
        });
    }
    Ok(truths)
}

/// Area-weighted resample of a row-major float image to [`SIZE`].
fn resample(source: &[[f32; 4]], size: [u32; 2]) -> Vec<[f32; 4]> {
    let (sw, sh) = (size[0] as usize, size[1] as usize);
    let (dw, dh) = (SIZE[0] as usize, SIZE[1] as usize);
    if (sw, sh) == (dw, dh) {
        return source.to_vec();
    }
    let (fx, fy) = (sw as f64 / dw as f64, sh as f64 / dh as f64);
    let mut out = Vec::with_capacity(dw * dh);
    for y in 0..dh {
        let (y0, y1) = (y as f64 * fy, (y + 1) as f64 * fy);
        for x in 0..dw {
            let (x0, x1) = (x as f64 * fx, (x + 1) as f64 * fx);
            let mut sum = [0.0f64; 4];
            let mut total = 0.0;
            for sy in y0.floor() as usize..(y1.ceil() as usize).min(sh) {
                let wy = (y1.min(sy as f64 + 1.) - y0.max(sy as f64)).max(0.);
                for sx in x0.floor() as usize..(x1.ceil() as usize).min(sw) {
                    let w = wy * (x1.min(sx as f64 + 1.) - x0.max(sx as f64)).max(0.);
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += w * f64::from(source[sy * sw + sx][c]);
                    }
                    total += w;
                }
            }
            out.push(sum.map(|v| (v / total.max(1e-12)) as f32));
        }
    }
    out
}

/// Linear HDR radiance error of a preset's accumulated radiance (`size`) against `truth`.
pub fn transport(radiance: &[[f32; 4]], size: [u32; 2], truth: &Truth) -> Result<Value> {
    let e = ara_pathtrace::metrics::radiance(&resample(radiance, size), &truth.radiance)?;
    Ok(json!({"rmse": e.rmse, "relativeRmse": e.relative_rmse, "mae": e.mae}))
}

/// Display errors of a capture (any size; downsampled to [`SIZE`]) against `truth`.
/// Writes the FLIP error map next to the captures.
pub fn score(capture: &RgbaImage, truth: &Truth, heatmap: &Path) -> Result<Value> {
    let test = if capture.dimensions() == (SIZE[0], SIZE[1]) {
        capture.clone()
    } else {
        imageops::resize(capture, SIZE[0], SIZE[1], imageops::FilterType::Lanczos3)
    };
    let mut square = 0.0f64;
    for (a, b) in test.pixels().zip(truth.image.pixels()) {
        for c in 0..3 {
            let d = (f64::from(a[c]) - f64::from(b[c])) / 255.;
            square += d * d;
        }
    }
    let rmse = (square / (f64::from(SIZE[0] * SIZE[1]) * 3.)).sqrt();
    let flip =
        ara_pathtrace::flip::flip(&truth.image, &test, ara_pathtrace::flip::PIXELS_PER_DEGREE)?;
    let map = RgbaImage::from_fn(SIZE[0], SIZE[1], |x, y| {
        let e = flip.errors[(y * SIZE[0] + x) as usize];
        // Black → red → yellow → white ramp.
        let v = [3. * e, 3. * e - 1., 3. * e - 2.].map(|c: f32| (c.clamp(0., 1.) * 255.) as u8);
        image::Rgba([v[0], v[1], v[2], 255])
    });
    map.save(heatmap)?;
    Ok(json!({"shot": truth.shot, "ev100": truth.ev100, "srgbRmse": rmse, "flip": flip.mean}))
}
