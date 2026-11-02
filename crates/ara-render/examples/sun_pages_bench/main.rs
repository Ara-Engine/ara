//! G2/G4 bench of the render-v3 sun page service (`Renderer::set_sun_pages`,
//! look::vsm::sun) against today's virtual shadow maps, on T1's lab recipes rebuilt in
//! `sun_pages_bench/fixture.rs` (T1's tooling is not merged into this branch).
//!
//! ```text
//! cargo run -p ara-render --example sun_pages_bench -- timing   # under the quiet-window lock
//! cargo run -p ara-render --example sun_pages_bench -- quality
//! cargo run -p ara-render --example sun_pages_bench -- g4 [spp]
//! ```
//!
//! - `timing`: every G2 case in three runs, `control` (today's map, High preset with the
//!   atlas at 4096 pages), `uncapped` (the service drawing all demand: the cost line) and
//!   `capped` (the service under the 4070 SUPER caps, 0.15 ms per layer: the lag line).
//!   Per frame: GPU ms of `VSM.Pages`, `VSM.Pages.Dynamic`, `VSM.Mark`, `VSM.Project`, the
//!   whole frame's pass sum, and the service's counters (T1 `StageCounters` fields).
//! - `quality`: the same runs; at sampled frames the capped and control captures against
//!   the uncapped capture of the same frame (the true-angle redraw): worst-region FLIP and
//!   the D29 temporal metric over consecutive sampled frames. PNGs at most 1080p.
//! - `g4`: the contact and pole-sweep cases, service and control, against
//!   `ara-render-fidelity`'s path-traced reference of the same geometry: worst-region FLIP,
//!   10-90% penumbra widths and edge displacement along the floor.
//!
//! Environment: `ARA_SUN_BENCH_SIZE` (`2560x1440`), `ARA_SUN_BENCH_CASES` (ids),
//! `ARA_SUN_BENCH_FRAMES` (a frame cap), `ARA_SUN_BENCH_OUT` (`target/sun-pages-bench`).
//! Outputs never go to tracked files.
mod fixture;
mod score;

use anyhow::{Context, Result};
use ara_gpu::{Access, GpuDevice, Resource};
use ara_render::{
    Renderer, SunPageCounters, SunPages,
    capture::{CaptureRequest, ViewMode},
    instanced::{TextureBinding, TextureStats, Textures},
};
use ara_render_fidelity::presets::QualityPreset;
use fixture::Case;
use serde::Serialize;
use std::path::{Path, PathBuf};

struct NoTextures;
impl Textures for NoTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn refine(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn binding(&self) -> Option<TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut GpuDevice) -> Result<()> {
        Ok(())
    }
}

/// Which shadows a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum Variant {
    Control,
    Uncapped,
    Capped,
}

impl Variant {
    fn pages(self) -> Option<SunPages> {
        match self {
            Self::Control => None,
            Self::Uncapped => Some(SunPages::uncapped()),
            Self::Capped => Some(SunPages::capped_4070s()),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Uncapped => "uncapped",
            Self::Capped => "capped",
        }
    }
}

/// One frame's measurements.
#[derive(Debug, Serialize)]
struct Frame {
    frame: u32,
    pages_ms: f64,
    dynamic_ms: f64,
    mark_ms: f64,
    project_ms: f64,
    /// Today's sun maps: cascades, far map and their moment pyramids (redrawn on any
    /// sun change; the control's moving-sun cost).
    maps_ms: f64,
    /// Sum of every pass this frame (not a complete-frame span).
    passes_ms: f64,
    counters: Option<SunPageCounters>,
}

#[derive(Debug, Serialize)]
struct Run {
    case: Case,
    variant: Variant,
    size: [u32; 2],
    device: String,
    frames: Vec<Frame>,
}

fn env_size() -> [u32; 2] {
    std::env::var("ARA_SUN_BENCH_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([2560, 1440])
}

fn out_dir() -> PathBuf {
    std::env::var("ARA_SUN_BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sun-pages-bench")
        })
}

fn selected(gate: u8) -> Vec<Case> {
    let filter = std::env::var("ARA_SUN_BENCH_CASES").ok();
    fixture::cases()
        .into_iter()
        .filter(|c| c.gates.contains(&gate))
        .filter(|c| {
            filter
                .as_ref()
                .is_none_or(|f| f.split(',').any(|id| id.trim() == c.id))
        })
        .collect()
}

/// A renderer for `f` at `size` with the High preset (no hardware rays; the atlas at
/// its 4096-page maximum so uncapped demand fits) and `variant`'s shadows.
fn renderer(
    f: &fixture::Fixture,
    size: [u32; 2],
    variant: Variant,
    cache: &Path,
) -> Result<Renderer> {
    let mut renderer = Renderer::new_native(&f.scene, Box::new(NoTextures), size, &[], cache)?;
    let mut quality = QualityPreset::High.raster_quality();
    quality.ray_queries = None;
    if let Some(v) = &mut quality.virtual_shadows {
        v.pages = 4096;
    }
    if direct() {
        // Direct sun only, against the reference's single scattering event.
        quality.ambient_occlusion = None;
        quality.reflections = None;
        quality.screen_gi = None;
    }
    renderer.set_quality(quality)?;
    let mut look = *renderer.look();
    look.auto_exposure = None;
    look.bloom = 0.;
    if direct() {
        look.sky_light = 0.;
    }
    renderer.set_look(look)?;
    renderer.set_jitter_override(Some([0.; 2]));
    renderer.set_tone_curve(ara_render::renderer::post::ToneCurve::Neutral);
    renderer.set_sun_pages(variant.pages())?;
    Ok(renderer)
}

/// Waits while another agent holds the overnight quiet-window lock (`ARA_QUIET_LOCK`),
/// so functional runs never load the GPU inside someone's timing window. The timing
/// mode runs inside B2's own window (`B2` in the lock) and does not wait for itself.
fn yield_to_timing() {
    let Ok(lock) = std::env::var("ARA_QUIET_LOCK") else {
        return;
    };
    while let Ok(owner) = std::fs::read_to_string(&lock) {
        if owner.starts_with("owner=B2 ") {
            return;
        }
        eprintln!("waiting for the quiet window: {}", owner.trim());
        std::thread::sleep(std::time::Duration::from_secs(20));
    }
}

/// Whether runs isolate direct sun light (`ARA_SUN_BENCH_DIRECT=1`): a black
/// environment, no AO, reflections or screen GI, and references of one scattering
/// event, so image lines compare shadows rather than ambient models.
fn direct() -> bool {
    std::env::var("ARA_SUN_BENCH_DIRECT").is_ok_and(|v| v == "1")
}

/// Frames a run of `case` scores (`ARA_SUN_BENCH_FRAMES` caps the script).
fn frames(case: &Case) -> u32 {
    std::env::var("ARA_SUN_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(case.frames, |cap: u32| cap.min(case.frames))
}

fn exposure(f: &fixture::Fixture) -> f32 {
    ara_render_fidelity::settings::exposure_from_ev100(f.ev100, 0.)
}

/// Runs `case` with `variant`: 16 warm frames at the first pose, then the script; the
/// frames in `samples` are captured (the displayed frame) and returned.
fn run(
    case: &Case,
    variant: Variant,
    size: [u32; 2],
    samples: &[u32],
) -> Result<(Run, Vec<(u32, image::RgbaImage)>)> {
    yield_to_timing();
    let mut f = fixture::create(case);
    if direct() {
        fixture::direct_only(&mut f);
    }
    let cache = out_dir().join("gpu-cache");
    let mut r = renderer(&f, size, variant, &cache)?;
    let exposure = exposure(&f);
    for _ in 0..16 {
        r.render(&f.camera, exposure)?;
    }
    let frames = frames(case);
    let mut run = Run {
        case: case.clone(),
        variant,
        size,
        device: r.gpu().info().name.clone(),
        frames: Vec::new(),
    };
    let mut captures = Vec::new();
    for frame in 0..frames {
        fixture::apply(&mut f, case, frame);
        r.update_native(&f.scene)?;
        r.render(&f.camera, exposure)?;
        let ms = |label: &str| -> f64 {
            r.timings()
                .iter()
                .filter(|t| t.pass == label)
                .map(|t| t.milliseconds)
                .sum()
        };
        run.frames.push(Frame {
            frame,
            pages_ms: ms("VSM.Pages"),
            dynamic_ms: ms("VSM.Pages.Dynamic"),
            mark_ms: ms("VSM.Mark"),
            project_ms: ms("VSM.Project"),
            maps_ms: r
                .timings()
                .iter()
                .filter(|t| {
                    t.pass == "ShadowAtlas.Cascades"
                        || t.pass == "ShadowAtlas.Far"
                        || t.pass.starts_with("ShadowMoments")
                })
                .map(|t| t.milliseconds)
                .sum(),
            passes_ms: r.timings().iter().map(|t| t.milliseconds).sum(),
            counters: r.sun_page_counters().cloned(),
        });
        if samples.contains(&frame) {
            let capture = r.capture_completed(&CaptureRequest {
                camera: f.camera,
                size,
                mode: ViewMode::Lit,
                deterministic: true,
                exposure,
                settle_frames: 0,
                tick: u64::from(frame),
                seed: 0,
                marks: None,
            })?;
            captures.push((frame, capture.image));
        }
    }
    r.gpu().assert_clean()?;
    Ok((run, captures))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

/// Saves `image` at most 1920x1080.
fn save_png(image: &image::RgbaImage, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if image.width() > 1920 || image.height() > 1080 {
        let scale = (1920. / image.width() as f32).min(1080. / image.height() as f32);
        image::imageops::resize(
            image,
            (image.width() as f32 * scale) as u32,
            (image.height() as f32 * scale) as u32,
            image::imageops::FilterType::Triangle,
        )
        .save(path)?;
    } else {
        image.save(path)?;
    }
    Ok(())
}

/// Average / p99 / max of `values` (nearest rank, as T1's contract).
fn spread(values: impl IntoIterator<Item = f64>) -> [f64; 3] {
    let mut v: Vec<f64> = values.into_iter().collect();
    if v.is_empty() {
        return [0.; 3];
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    [
        v.iter().sum::<f64>() / n as f64,
        v[(n * 99).div_ceil(100) - 1],
        v[n - 1],
    ]
}

fn timing() -> Result<()> {
    let size = env_size();
    let out = out_dir().join("timing");
    let mut summary = String::from(
        "| case | run | pages ms avg/p99/max | dynamic ms avg/p99/max | mark ms avg/max | project ms avg/max | sun maps ms avg/max | static demand avg/max | debt max | lag max (near) texels | deadline misses |\n|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for case in selected(2) {
        for variant in [Variant::Control, Variant::Uncapped, Variant::Capped] {
            let (run, _) = run(&case, variant, size, &[])?;
            write_json(
                &out.join(format!("{}-{}.json", case.id, variant.name())),
                &run,
            )?;
            // Scored frames start at the event (frame 8); warm-up excluded.
            let scored: Vec<&Frame> = run
                .frames
                .iter()
                .filter(|f| f.frame >= case.event_frame)
                .collect();
            let pages = spread(scored.iter().map(|f| f.pages_ms));
            let dynamic = spread(scored.iter().map(|f| f.dynamic_ms));
            let mark = spread(scored.iter().map(|f| f.mark_ms));
            let project = spread(scored.iter().map(|f| f.project_ms));
            let maps = spread(scored.iter().map(|f| f.maps_ms));
            let counters: Vec<&SunPageCounters> =
                scored.iter().filter_map(|f| f.counters.as_ref()).collect();
            let demand = spread(
                counters
                    .iter()
                    .map(|c| c.sun_pages.demand.unwrap_or(0) as f64),
            );
            let debt = counters
                .iter()
                .map(|c| c.sun_pages.debt.unwrap_or(0) + c.dynamic_pages.debt.unwrap_or(0))
                .max()
                .unwrap_or(0);
            let lag = counters
                .iter()
                .map(|c| {
                    c.sun_pages
                        .edge_lag_page_texels
                        .unwrap_or(0.)
                        .max(c.dynamic_pages.edge_lag_page_texels.unwrap_or(0.))
                })
                .fold(0., f64::max);
            let near = counters
                .iter()
                .map(|c| {
                    c.sun_pages
                        .near_edge_lag_page_texels
                        .unwrap_or(0.)
                        .max(c.dynamic_pages.near_edge_lag_page_texels.unwrap_or(0.))
                })
                .fold(0., f64::max);
            let misses: u64 = counters
                .iter()
                .map(|c| {
                    c.sun_pages.deadline_misses.unwrap_or(0)
                        + c.dynamic_pages.deadline_misses.unwrap_or(0)
                })
                .sum();
            let line = format!(
                "| {} | {} | {:.3}/{:.3}/{:.3} | {:.3}/{:.3}/{:.3} | {:.3}/{:.3} | {:.3}/{:.3} | {:.3}/{:.3} | {:.0}/{:.0} | {} | {:.3} ({:.3}) | {} |\n",
                case.id,
                variant.name(),
                pages[0],
                pages[1],
                pages[2],
                dynamic[0],
                dynamic[1],
                dynamic[2],
                mark[0],
                mark[2],
                project[0],
                project[2],
                maps[0],
                maps[2],
                demand[0],
                demand[2],
                debt,
                lag,
                near,
                misses
            );
            print!("{line}");
            summary.push_str(&line);
        }
    }
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("summary.md"), summary)?;
    Ok(())
}

fn quality() -> Result<()> {
    let size = env_size();
    let out = out_dir().join("quality");
    let mut summary = String::from(
        "| case | frame | capped FLIP worst / whole vs uncapped | control FLIP worst / whole vs uncapped | capped lag max texels |\n|---|---|---|---|---|\n",
    );
    let mut temporal = String::from(
        "| case | run | D29 max 16x16 RMS (white) vs uncapped, frames F..F+4 |\n|---|---|---|\n",
    );
    let mut absolute = String::from(
        "| case | frame | uncapped FLIP worst / whole vs path-traced | capped | control |\n|---|---|---|---|---|\n",
    );
    for case in selected(2) {
        let e = case.event_frame;
        let n = frames(&case);
        let mut samples: Vec<u32> = [
            e,
            e + 1,
            e + 2,
            e + 3,
            e + 4,
            e + 60,
            e + 120,
            e + 180,
            e + 240,
            n - 1,
        ]
        .into_iter()
        .filter(|f| *f < n)
        .collect();
        samples.dedup();
        let mut images = std::collections::BTreeMap::new();
        let mut lags = std::collections::BTreeMap::new();
        for variant in [Variant::Uncapped, Variant::Capped, Variant::Control] {
            let (run, captures) = run(&case, variant, size, &samples)?;
            for f in &run.frames {
                if let Some(c) = &f.counters {
                    lags.insert(
                        (variant, f.frame),
                        c.sun_pages
                            .edge_lag_page_texels
                            .unwrap_or(0.)
                            .max(c.dynamic_pages.edge_lag_page_texels.unwrap_or(0.)),
                    );
                }
            }
            for (frame, image) in captures {
                save_png(
                    &image,
                    &out.join(&case.id)
                        .join(format!("{}-{frame:04}.png", variant.name())),
                )?;
                images.insert((variant, frame), image);
            }
        }
        for &frame in &samples {
            let reference = &images[&(Variant::Uncapped, frame)];
            let capped = score::flip_regions(reference, &images[&(Variant::Capped, frame)])?;
            let control = score::flip_regions(reference, &images[&(Variant::Control, frame)])?;
            let line = format!(
                "| {} | {frame} | {:.4} / {:.4} | {:.4} / {:.4} | {:.3} |\n",
                case.id,
                capped.worst,
                capped.whole,
                control.worst,
                control.whole,
                lags.get(&(Variant::Capped, frame)).copied().unwrap_or(0.)
            );
            print!("{line}");
            summary.push_str(&line);
        }
        for variant in [Variant::Capped, Variant::Control] {
            let mut worst: f64 = 0.;
            for frame in e + 1..=e + 4 {
                if frame >= n {
                    break;
                }
                worst = worst.max(score::temporal(
                    (&images[&(variant, frame)], &images[&(variant, frame - 1)]),
                    (
                        &images[&(Variant::Uncapped, frame)],
                        &images[&(Variant::Uncapped, frame - 1)],
                    ),
                ));
            }
            temporal.push_str(&format!(
                "| {} | {} | {worst:.5} |\n",
                case.id,
                variant.name()
            ));
        }
        // The true-angle image against the path tracer at F+60 (sun and city cases).
        let r = e + 60;
        if matches!(case.recipe, "sun" | "city") && samples.contains(&r) {
            let mut f = fixture::create(&case);
            if direct() {
                fixture::direct_only(&mut f);
            }
            fixture::apply(&mut f, &case, r);
            yield_to_timing();
            let mut gpu = GpuDevice::new(&[], out_dir().join("reference-cache"))?;
            let reference = score::reference(&mut gpu, &f, size, reference_spp())?;
            gpu.assert_clean()?;
            drop(gpu);
            save_png(
                &reference,
                &out.join(&case.id).join(format!("reference-{r:04}.png")),
            )?;
            let mut row = format!("| {} | {r} |", case.id);
            for variant in [Variant::Uncapped, Variant::Capped, Variant::Control] {
                let flip = score::flip_regions(&reference, &images[&(variant, r)])?;
                row.push_str(&format!(" {:.4} / {:.4} |", flip.worst, flip.whole));
            }
            println!("{row}");
            absolute.push_str(&row);
            absolute.push('\n');
        }
    }
    std::fs::create_dir_all(&out)?;
    std::fs::write(
        out.join("summary.md"),
        format!("{summary}\n{temporal}\n{absolute}"),
    )?;
    Ok(())
}

/// Samples per pixel of the G2 path-traced references (`ARA_SUN_BENCH_SPP`, 128).
fn reference_spp() -> u32 {
    std::env::var("ARA_SUN_BENCH_SPP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128)
}

fn g4(spp: u32) -> Result<()> {
    let size = env_size();
    let out = out_dir().join("g4");
    let cases = selected(4);
    // Candidates first (one renderer device at a time), then one reference device.
    let mut candidates = Vec::new();
    for case in &cases {
        let last = frames(case) - 1;
        // Rect-light cases do not use sun pages: today's control only.
        let sun = matches!(case.recipe, "contact_sun" | "pole_sweep");
        let variants: &[Variant] = if sun {
            &[Variant::Uncapped, Variant::Control]
        } else {
            &[Variant::Control]
        };
        for &variant in variants {
            let (_, captures) = run(case, variant, size, &[last])?;
            let (_, image) = captures
                .into_iter()
                .next()
                .context("captured the last frame")?;
            save_png(
                &image,
                &out.join(&case.id).join(format!("{}.png", variant.name())),
            )?;
            candidates.push((case.clone(), variant, image));
        }
    }
    let mut gpu = GpuDevice::new(&[], out_dir().join("reference-cache"))?;
    let mut summary = String::from(
        "| case | run | worst-region FLIP | whole FLIP | penumbra width rel. error (left / right) | edge displacement px (left / right) |\n|---|---|---|---|---|---|\n",
    );
    for case in &cases {
        let mut f = fixture::create(case);
        if direct() {
            fixture::direct_only(&mut f);
        }
        fixture::apply(&mut f, case, frames(case) - 1);
        yield_to_timing();
        let reference = score::reference(&mut gpu, &f, size, spp)?;
        save_png(&reference, &out.join(&case.id).join("reference.png"))?;
        for (c, variant, image) in candidates.iter().filter(|(c, _, _)| c.id == case.id) {
            let flip = score::flip_regions(&reference, image)?;
            let widths = score::penumbrae(&f, c, size, image, &reference);
            let line = format!(
                "| {} | {} | {:.4} | {:.4} | {} | {} |\n",
                case.id,
                variant.name(),
                flip.worst,
                flip.whole,
                widths.map_or("n/a".into(), |w| format!(
                    "{:+.1}% / {:+.1}%",
                    w.width[0] * 100.,
                    w.width[1] * 100.
                )),
                widths.map_or("n/a".into(), |w| format!(
                    "{:.2} / {:.2}",
                    w.edge_px[0], w.edge_px[1]
                )),
            );
            print!("{line}");
            summary.push_str(&line);
        }
    }
    gpu.assert_clean()?;
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("summary.md"), summary)?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("timing") => timing(),
        Some("quality") => quality(),
        Some("g4") => g4(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(256)),
        _ => anyhow::bail!("usage: sun_pages_bench timing | quality | g4 [spp]"),
    }
}
