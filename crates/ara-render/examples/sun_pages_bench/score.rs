//! Scoring for the sun page bench: FLIP over T1's fixed regions (whole image and a 4x4
//! partition), the D29 temporal metric on display-linear luminance, the path-traced
//! reference of a fixture, and penumbra profiles along the floor.
use super::fixture::{Case, Fixture};
use anyhow::Result;
use ara_gpu::GpuDevice;
use ara_render::scene::CameraSpec;
use ara_render_fidelity::{
    FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, ToneMapper, Upscaler,
};
use glam::{Mat4, Vec3};
use image::RgbaImage;

/// Mean FLIP of the whole image and of its worst region of a fixed 4x4 partition.
pub(crate) struct Flip {
    pub whole: f64,
    pub worst: f64,
}

/// FLIP of `test` against `reference` (67 pixels per degree, the paper's viewing).
pub(crate) fn flip_regions(reference: &RgbaImage, test: &RgbaImage) -> Result<Flip> {
    let r = ara_pathtrace::flip::flip(reference, test, ara_pathtrace::flip::PIXELS_PER_DEGREE)?;
    let (w, h) = (reference.width() as usize, reference.height() as usize);
    let mut worst: f64 = 0.;
    for ry in 0..4 {
        for rx in 0..4 {
            let (x0, x1) = (rx * w / 4, (rx + 1) * w / 4);
            let (y0, y1) = (ry * h / 4, (ry + 1) * h / 4);
            let mut sum = 0.;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += f64::from(r.errors[y * w + x]);
                }
            }
            worst = worst.max(sum / ((x1 - x0) * (y1 - y0)) as f64);
        }
    }
    Ok(Flip {
        whole: r.mean,
        worst,
    })
}

fn linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(p: &image::Rgba<u8>) -> f64 {
    0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2])
}

/// D29's reference-subtracted temporal metric: the largest 16x16-block RMS of
/// `(test[t] - test[t-1]) - (reference[t] - reference[t-1])`, display-linear luminance
/// (display white = 1; the bench reads captures, not radiance).
pub(crate) fn temporal(test: (&RgbaImage, &RgbaImage), reference: (&RgbaImage, &RgbaImage)) -> f64 {
    let (w, h) = test.0.dimensions();
    let mut worst: f64 = 0.;
    for by in (0..h).step_by(16) {
        for bx in (0..w).step_by(16) {
            let (mut sum, mut n) = (0., 0.);
            for y in by..(by + 16).min(h) {
                for x in bx..(bx + 16).min(w) {
                    let d = (luminance(test.0.get_pixel(x, y)) - luminance(test.1.get_pixel(x, y)))
                        - (luminance(reference.0.get_pixel(x, y))
                            - luminance(reference.1.get_pixel(x, y)));
                    sum += d * d;
                    n += 1.;
                }
            }
            worst = worst.max((sum / n).sqrt());
        }
    }
    worst
}

/// The fixture's path-traced reference at `spp` samples (8 bounces), through
/// `ara-render-fidelity`'s reference mode over the same geometry, with the renderer's
/// exposure and the PBR Neutral display curve. A finite, undenoised budget.
pub(crate) fn reference(
    gpu: &mut GpuDevice,
    f: &Fixture,
    size: [u32; 2],
    spp: u32,
) -> Result<RgbaImage> {
    let scene = super::fixture::legacy(f);
    let settings = Settings {
        render_size: size,
        display_size: size,
        tier: Tier::Rt,
        mode: Mode::Reference {
            samples: spp,
            bounces: 8,
        },
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 1.,
        post: PostSettings {
            ev100: Some(f.ev100),
            tone_mapper: ToneMapper::PbrNeutral,
            bloom: 0.,
            fog_density: 0.,
            aperture: 0.,
            shutter: 0.,
            ..PostSettings::default()
        },
    };
    let mut view = FidelityView::new(gpu, &scene, settings, None)?;
    let mut reset = true;
    while view.reference_samples() < spp {
        view.render(
            gpu,
            &FrameInput {
                camera: &f.camera,
                tick: 0,
                delta_seconds: 1. / 60.,
                reset,
                lighting_changed: false,
                deterministic: true,
            },
        )?;
        reset = false;
    }
    let image = view.capture(gpu)?;
    view.destroy(gpu)?;
    Ok(image)
}

/// Penumbra comparison on the floor: relative 10-90% width error and the 50% edge's
/// displacement in pixels, on two lines across the plate shadow's camera-side edge.
#[derive(Clone, Copy)]
pub(crate) struct Penumbra {
    pub width: [f64; 2],
    pub edge_px: [f64; 2],
}

/// The camera's pixel of world `p` at `size` (vertical field of view, y up).
fn pixel(camera: &CameraSpec, size: [u32; 2], p: Vec3) -> Option<[f64; 2]> {
    let CameraSpec::LookAt {
        position,
        target,
        vertical_fov,
    } = *camera
    else {
        return None;
    };
    let view = Mat4::look_at_rh(position, target, Vec3::Y);
    let proj = Mat4::perspective_rh(vertical_fov, size[0] as f32 / size[1] as f32, 0.05, 1000.);
    let ndc = (proj * view).project_point3(p);
    Some([
        f64::from((ndc.x + 1.) * 0.5 * size[0] as f32),
        f64::from((1. - ndc.y) * 0.5 * size[1] as f32),
    ])
}

fn sample(image: &RgbaImage, [x, y]: [f64; 2]) -> Option<f64> {
    let (x, y) = (x - 0.5, y - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    if x0 < 0.
        || y0 < 0.
        || x0 + 1. >= f64::from(image.width())
        || y0 + 1. >= f64::from(image.height())
    {
        return None;
    }
    let (fx, fy) = (x - x0, y - y0);
    let at = |dx: u32, dy: u32| luminance(image.get_pixel(x0 as u32 + dx, y0 as u32 + dy));
    Some(
        at(0, 0) * (1. - fx) * (1. - fy)
            + at(1, 0) * fx * (1. - fy)
            + at(0, 1) * (1. - fx) * fy
            + at(1, 1) * fx * fy,
    )
}

/// 10%, 50% and 90% crossings (world z) of a profile running from umbra to lit.
fn crossings(zs: &[f64], values: &[f64]) -> Option<[f64; 3]> {
    let n = values.len();
    let tail = (n / 10).max(1);
    let umbra = values[..tail].iter().sum::<f64>() / tail as f64;
    let lit = values[n - tail..].iter().sum::<f64>() / tail as f64;
    if lit - umbra < 0.02 {
        return None;
    }
    let mut out = [f64::NAN; 3];
    for (k, level) in [0.1, 0.5, 0.9].into_iter().enumerate() {
        for i in 1..n {
            let (a, b) = (
                (values[i - 1] - umbra) / (lit - umbra),
                (values[i] - umbra) / (lit - umbra),
            );
            if a < level && b >= level {
                out[k] = zs[i - 1] + (zs[i] - zs[i - 1]) * (level - a) / (b - a);
                break;
            }
        }
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// Penumbrae of the contact cases' plate shadow (the edge facing the camera, from the
/// plate's z = +0.5 edge, on two lines inside the shadow's length), test against
/// reference; `None` for other recipes or unmeasurable (sub-pixel or occluded) edges.
pub(crate) fn penumbrae(
    f: &Fixture,
    case: &Case,
    size: [u32; 2],
    test: &RgbaImage,
    reference: &RgbaImage,
) -> Option<Penumbra> {
    let h = f64::from(case.contact_m);
    // The edge's floor z, two lines across it and the profile's half-length.
    let (edge, lines, span) = match case.recipe {
        // The sun at azimuth 0 shifts the shadow along -x only.
        "contact_sun" => (0.5, [-h - 0.4, -h + 0.4], 0.02 + 0.03 * h),
        // The rect light at (-1, h + 4, 1) projects from its centre.
        "contact_rect" => {
            let s = (h + 4.) / 4.;
            (
                1. - 0.5 * s,
                [-1. + 0.6 * s, -1. + 1.4 * s],
                0.05 + 0.15 * h,
            )
        }
        _ => return None,
    };
    let mut out = Penumbra {
        width: [f64::NAN; 2],
        edge_px: [f64::NAN; 2],
    };
    for (line, x) in lines.into_iter().enumerate() {
        let zs: Vec<f64> = (0..=400)
            .map(|i| edge - span + 2. * span * f64::from(i) / 400.)
            .collect();
        let profile = |image: &RgbaImage| -> Option<Vec<f64>> {
            zs.iter()
                .map(|&z| {
                    sample(
                        image,
                        pixel(&f.camera, size, Vec3::new(x as f32, 0., z as f32))?,
                    )
                })
                .collect()
        };
        let (Some(a), Some(b)) = (profile(test), profile(reference)) else {
            continue;
        };
        let (Some(ca), Some(cb)) = (crossings(&zs, &a), crossings(&zs, &b)) else {
            continue;
        };
        let (wa, wb) = (ca[2] - ca[0], cb[2] - cb[0]);
        out.width[line] = wa / wb - 1.;
        let pa = pixel(&f.camera, size, Vec3::new(x as f32, 0., ca[1] as f32))?;
        let pb = pixel(&f.camera, size, Vec3::new(x as f32, 0., cb[1] as f32))?;
        out.edge_px[line] = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2)).sqrt();
    }
    out.width.iter().any(|v| v.is_finite()).then_some(out)
}
