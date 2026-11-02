//! The G1 bench (ignored tests; T1's lab tooling is not merged into this branch, so the
//! G1 scenes are rebuilt here, `scene`). Outputs go under `target/lights-bench/`.
//!
//! - `g1_quality`: every case at the bench size: V2 and the exact finite-range
//!   control, with and without blockers, E3's lines (`score`), lights per pixel,
//!   rays per pixel, the overload candidate's omitted-energy bound, and PNGs.
//! - `g1_timing`: every case, V2 alone, warm-up then measured frames: GPU timestamps
//!   of build (+ bin), prep, select and shade, p50 / p99 / max. Run under the
//!   overnight quiet-window lock.
//! - `g1_export`: each static case's frame zero for the path-traced reference
//!   (`ara-pathtrace` example `lights_v2_reference`): `scene.json` and RGBA32F
//!   radiance files (V2, exact control, V2 with omission, today's `look::local`
//!   control), plus the control's pass timings.
//!
//! Environment: `ARA_LIGHTS_BENCH_CASES` (comma-separated ids, default all),
//! `ARA_LIGHTS_BENCH_SIZE` (`2560x1440`), `ARA_LIGHTS_BENCH_FRAMES` (quality 60,
//! timing 300), `ARA_LIGHTS_BENCH_SAMPLES` (16), `ARA_LIGHTS_BENCH_OUT`.
mod contract;
mod control;
mod harness;
pub(crate) mod scene;
mod score;

use super::Settings;
use anyhow::Result;
pub(crate) use harness::{Bench, Blockers, Readback, Timing};
use scene::{Case, Layout, Motion};
use serde::Serialize;
use std::path::PathBuf;

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn size() -> [u32; 2] {
    std::env::var("ARA_LIGHTS_BENCH_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([2560, 1440])
}

fn out_dir() -> PathBuf {
    std::env::var("ARA_LIGHTS_BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/lights-bench")
        })
}

/// Whether `ARA_LIGHTS_BENCH_CASES` (comma-separated ids; unset: all) names `id`.
fn wanted(id: &str) -> bool {
    std::env::var("ARA_LIGHTS_BENCH_CASES")
        .ok()
        .is_none_or(|f| f.split(',').any(|c| c.trim() == id))
}

fn selected() -> Vec<Case> {
    let filter = std::env::var("ARA_LIGHTS_BENCH_CASES").ok();
    scene::cases()
        .into_iter()
        .filter(|c| {
            filter
                .as_ref()
                .is_none_or(|f| f.split(',').any(|id| id.trim() == c.id))
        })
        .collect()
}

fn settings() -> Settings {
    Settings {
        samples: env_or("ARA_LIGHTS_BENCH_SAMPLES", 16),
        ..Settings::default()
    }
}

/// Pool entries for a case: generous, so tiles never fall back.
fn entries(case: &Case, size: [u32; 2]) -> u32 {
    let tiles = size[0].div_ceil(16) * size[1].div_ceil(16);
    tiles * if case.target_lpp > 10. { 384 } else { 96 }
}

/// Percentiles as R5-1 reports them (p99 = sorted[floor(0.99 (n - 1))]).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct Spread {
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
}

impl Spread {
    pub(crate) fn of(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut v = values.to_vec();
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v[((q * (v.len() - 1) as f64).floor()) as usize];
        Self {
            p50: at(0.5),
            p99: at(0.99),
            max: v[v.len() - 1],
            mean: v.iter().sum::<f64>() / v.len() as f64,
        }
    }
}

/// One case's quality receipt.
#[derive(Debug, Serialize)]
struct Quality {
    case: String,
    size: [u32; 2],
    samples: u32,
    omission: Option<f32>,
    frames: u32,
    e3: score::E3,
    /// E3 over the frames whose exact control reproduced on its first re-run.
    e3_reproduced: score::E3,
    e3_reproduced_exact: bool,
    /// Frames whose exact control needed retries, and the extra exact runs in all.
    irreproducible_frames: Vec<u32>,
    control_retries: u32,
    e3_exact: bool,
    highlight_retention: f64,
    shadow_retention: f64,
    /// Lights reaching each covered pixel (positive shading, before visibility).
    lpp_covered: f64,
    /// Over the whole screen.
    lpp_screen: f64,
    /// Segments traced per covered pixel.
    rays_per_pixel: f64,
    /// (Pixel, light) pairs per covered pixel whose visibility the sample masks resolved.
    masked_pairs_per_pixel: f64,
    /// Largest per-pixel (control - test) / control luminance (omission's line).
    max_omitted_fraction: f64,
    /// Pixel-lights omitted per covered pixel.
    omitted_per_pixel: f64,
    stats: Vec<harness_stats::Row>,
}

mod harness_stats {
    use serde::Serialize;
    /// Select's counters for one frame.
    #[derive(Debug, Serialize)]
    pub(super) struct Row {
        pub entries: u32,
        pub partial: u32,
        pub all: u32,
        pub classification_tests: u32,
        pub clean: bool,
    }
}

fn row(t: &Timing) -> harness_stats::Row {
    harness_stats::Row {
        entries: t.stats.entries,
        partial: t.stats.partial,
        all: t.stats.all,
        classification_tests: t.stats.classification_tests,
        clean: t.stats.clean(),
    }
}

/// The exact control's image of the posed frame, taken once two runs agree bit for bit
/// (the first is already in `exact`; up to five runs). The exact pass is occasionally
/// not reproducible on this device (about one run in ten differs everywhere; V2 never
/// did): `retries` counts the extra runs this needed.
fn agreed(
    bench: &mut Bench,
    blockers: Blockers,
    settings: Settings,
    retries: &mut u32,
) -> Result<Vec<f32>> {
    let mut previous = bench.read(Readback::Exact)?;
    for _ in 0..4 {
        bench.run_exact(false, blockers, settings)?;
        let next = bench.read(Readback::Exact)?;
        let same = previous
            .iter()
            .zip(&next)
            .all(|(a, b)| a.to_bits() == b.to_bits());
        if same {
            return Ok(next);
        }
        *retries += 1;
        previous = next;
    }
    anyhow::bail!("the exact control did not reproduce in five runs")
}

/// Scores `frames` frames of `case` (E3 dynamics where the case has them).
pub(crate) fn quality(
    case: &Case,
    size: [u32; 2],
    frames: u32,
    settings: Settings,
    images: bool,
) -> Result<serde_json::Value> {
    let layout = Layout::new(case);
    let ev100 = layout.ev100;
    let mut bench = Bench::new(layout, size, entries(case, size))?;
    let (w, h) = (size[0] as usize, size[1] as usize);
    let mut e3 = score::E3::default();
    // The same lines over the frames whose exact control reproduced at once.
    let mut e3_reproduced = score::E3::default();
    let mut irreproducible = Vec::new();
    let (mut lit, mut rays, mut covered_px, mut omitted) = (0f64, 0f64, 0f64, 0f64);
    let mut masked = 0f64;
    let mut max_omitted = 0f64;
    let mut stats = Vec::new();
    let dir = out_dir().join(&case.id);
    let shadows = true;
    let mut retries = 0u32;
    for f in 0..frames {
        bench.pose(f)?;
        let before = retries;
        // V2 and the exact control share one submission (identical inputs).
        let timing = bench.run_pair(true, Blockers::On, settings)?;
        anyhow::ensure!(
            timing.stats.clean(),
            "{} frame {f}: tiles fell back: {:?}",
            case.id,
            timing.stats
        );
        stats.push(row(&timing));
        let test = bench.read(Readback::V2)?;
        let counters = bench.read(Readback::Counters)?;
        let material = bench.read(Readback::Material)?;
        let control = agreed(&mut bench, Blockers::On, settings, &mut retries)?;
        let (test_open, control_open) = if shadows {
            bench.run_pair(false, Blockers::Off, settings)?;
            let t = bench.read(Readback::V2)?;
            (
                Some(t),
                Some(agreed(&mut bench, Blockers::Off, settings, &mut retries)?),
            )
        } else {
            (None, None)
        };
        let covered: Vec<bool> = (0..w * h).map(|i| material[i * 4 + 2] > 0.5).collect();
        let slot = |i: usize| (material[i * 4 + 3] * 255.).round() as u32;
        let metal: Vec<bool> = (0..w * h)
            .map(|i| covered[i] && slot(i) == scene::GLOSSY)
            .collect();
        let blocker: Vec<bool> = (0..w * h)
            .map(|i| covered[i] && slot(i) != scene::FLOOR)
            .collect();
        let scored = score::Images {
            test: &test,
            control: &control,
            test_open: test_open.as_deref(),
            control_open: control_open.as_deref(),
            covered: &covered,
            metal: &metal,
            blocker: &blocker,
        };
        e3.add(w, h, &scored);
        if retries == before {
            e3_reproduced.add(w, h, &scored);
        } else {
            irreproducible.push(f);
            e3_reproduced.reset_temporal();
        }
        for i in (0..w * h).filter(|&i| covered[i]) {
            lit += f64::from(counters[i * 4]);
            // Rays plus 4096 per mask-resolved pair (lights-v2.slangh LIGHTS_MASKED).
            let work = f64::from(counters[i * 4 + 1]);
            masked += (work / 4096.).floor();
            rays += work % 4096.;
            omitted += f64::from(counters[i * 4 + 3]);
            covered_px += 1.;
            let lum = |p: &[f32]| {
                0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
            };
            let (lc, lt) = (lum(&control[i * 4..]), lum(&test[i * 4..]));
            if lc > 0. {
                max_omitted = max_omitted.max((lc - lt) / lc);
            }
        }
        if images && (f == 0 || f + 1 == frames) {
            let tag = if settings.omission.is_some() {
                "-omit"
            } else {
                ""
            };
            score::png(
                &dir.join(format!("v2{tag}-f{f:02}.png")),
                &test,
                size,
                ev100,
            )?;
            score::png(
                &dir.join(format!("exact-f{f:02}.png")),
                &control,
                size,
                ev100,
            )?;
            let error = |i: usize| {
                let a = 0.2126 * (test[i * 4] - control[i * 4])
                    + 0.7152 * (test[i * 4 + 1] - control[i * 4 + 1])
                    + 0.0722 * (test[i * 4 + 2] - control[i * 4 + 2]);
                let b = 0.2126 * control[i * 4]
                    + 0.7152 * control[i * 4 + 1]
                    + 0.0722 * control[i * 4 + 2];
                a.abs() / b.max(1e-6)
            };
            score::heat_png(
                &dir.join(format!("error{tag}-f{f:02}.png")),
                size,
                0.02,
                error,
            )?;
            score::heat_png(&dir.join(format!("lpp-f{f:02}.png")), size, 40., |i| {
                counters[i * 4]
            })?;
            score::heat_png(
                &dir.join(format!("rays{tag}-f{f:02}.png")),
                size,
                256.,
                |i| counters[i * 4 + 1] % 4096.,
            )?;
            score::heat_png(
                &dir.join(format!("masked{tag}-f{f:02}.png")),
                size,
                16.,
                |i| (counters[i * 4 + 1] / 4096.).floor(),
            )?;
        }
    }
    bench.destroy()?;
    let q = Quality {
        case: case.id.clone(),
        size,
        samples: settings.samples,
        omission: settings.omission,
        frames,
        e3_exact: e3.exact(),
        e3_reproduced_exact: e3_reproduced.exact(),
        e3_reproduced,
        irreproducible_frames: irreproducible,
        control_retries: retries,
        highlight_retention: e3.highlight_retention(),
        shadow_retention: e3.shadow_retention(),
        e3,
        lpp_covered: lit / covered_px.max(1.),
        lpp_screen: lit / (f64::from(frames) * (w * h) as f64),
        rays_per_pixel: rays / covered_px.max(1.),
        masked_pairs_per_pixel: masked / covered_px.max(1.),
        max_omitted_fraction: max_omitted,
        omitted_per_pixel: omitted / covered_px.max(1.),
        stats,
    };
    Ok(serde_json::to_value(q)?)
}

/// Times `frames` frames of V2 (after `warm` frames): per-stage spreads.
pub(crate) fn timing(
    case: &Case,
    size: [u32; 2],
    warm: u32,
    frames: u32,
    settings: Settings,
    shadowed: bool,
) -> Result<serde_json::Value> {
    let layout = Layout::new(case);
    let mut bench = Bench::new(layout, size, entries(case, size))?;
    let mut series: [Vec<f64>; 7] = Default::default();
    let mut first = None;
    for f in 0..warm + frames {
        let frame = if case.motion == Motion::Static {
            0
        } else {
            f % 300
        };
        bench.pose(frame)?;
        let t = bench.run_timed(settings, shadowed)?;
        anyhow::ensure!(
            t.stats.clean(),
            "{} frame {f}: tiles fell back: {:?}",
            case.id,
            t.stats
        );
        if f < warm {
            continue;
        }
        if first.is_none() {
            first = Some(row(&t));
        }
        let build = t.ms(&["Lights.Build", "Lights.Bin"]);
        let prep = t.ms(&["Lights.Prep"]);
        let select = t.ms(&["Lights.Select"]);
        // Shade is the combined pass, or the lean pass plus the visibility pass.
        let shade = t.ms(&["Lights.Shade", "Lights.Visibility"]);
        let visibility = t.ms(&["Lights.Visibility"]);
        for (s, v) in series.iter_mut().zip([
            build,
            prep,
            select,
            shade,
            build + prep + select + shade,
            t.ms(&["Bench.Primary"]),
            visibility,
        ]) {
            s.push(v);
        }
    }
    bench.destroy()?;
    let [build, prep, select, shade, total, primary, visibility] = series.map(|s| Spread::of(&s));
    Ok(serde_json::json!({
        "case": case.id, "size": size, "samples": settings.samples, "omission": settings.omission,
        "shadowed": shadowed, "classify": settings.classify,
        "frames": frames, "warm": warm,
        "build_ms": build, "prep_ms": prep, "select_ms": select, "shade_ms": shade, "total_ms": total,
        "visibility_part_ms": visibility,
        "bench_primary_ms": primary, "select_counters": first,
    }))
}

fn write(name: &str, value: &serde_json::Value) -> Result<()> {
    let dir = out_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(name), serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

#[test]
#[ignore = "G1 bench: minutes of GPU work at 2560x1440; run explicitly"]
fn g1_quality() -> Result<()> {
    let size = size();
    let frames = env_or("ARA_LIGHTS_BENCH_FRAMES", 60);
    let mut all = Vec::new();
    for case in selected() {
        let frames = if case.motion == Motion::Static {
            2
        } else {
            frames
        };
        let mut s = settings();
        all.push(quality(&case, size, frames, s, true)?);
        eprintln!("{}", all.last().map(|v| v.to_string()).unwrap_or_default());
        if case.target_lpp > 10. {
            s.omission = Some(0.01);
            all.push(quality(&case, size, frames, s, true)?);
            eprintln!("{}", all.last().map(|v| v.to_string()).unwrap_or_default());
        }
    }
    write("quality.json", &serde_json::Value::Array(all))
}

#[test]
#[ignore = "G1 bench timing: run under the quiet-window lock"]
fn g1_timing() -> Result<()> {
    let size = size();
    let frames = env_or("ARA_LIGHTS_BENCH_FRAMES", 300);
    let mut all = Vec::new();
    // Variants: `full` (V2 as built), `unshadowed` (shading alone), `noclassify`
    // (every pair traced through the whole TLAS), `omit` (overload, dense cases).
    let variants: Vec<String> = std::env::var("ARA_LIGHTS_BENCH_VARIANTS")
        .unwrap_or_else(|_| "full,unshadowed,omit".into())
        .split(',')
        .map(|v| v.trim().to_owned())
        .collect();
    let has = |v: &str| variants.iter().any(|x| x == v);
    for case in selected() {
        let s = settings();
        let dense = case.target_lpp > 10.;
        let mut runs = Vec::new();
        if has("full") {
            runs.push((s, true));
        }
        if has("unshadowed") {
            runs.push((s, false));
        }
        if has("noclassify") {
            runs.push((
                Settings {
                    classify: false,
                    ..s
                },
                true,
            ));
        }
        if has("omit") && dense {
            runs.push((
                Settings {
                    omission: Some(0.01),
                    ..s
                },
                true,
            ));
        }
        for (s, shadowed) in runs {
            all.push(timing(&case, size, 20, frames, s, shadowed)?);
            eprintln!("{}", all.last().map(|v| v.to_string()).unwrap_or_default());
        }
    }
    write("timing.json", &serde_json::Value::Array(all))
}

fn write_f32(path: &std::path::Path, rgba: &[f32]) -> Result<()> {
    let bytes: Vec<u8> = rgba.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Per-pass spreads of a control run's frames.
fn pass_spreads(frames: &[Vec<(String, f64)>]) -> serde_json::Value {
    let mut by: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    for frame in frames {
        let mut sums: std::collections::BTreeMap<&str, f64> = Default::default();
        for (pass, ms) in frame {
            *sums.entry(pass).or_default() += ms;
        }
        for (pass, ms) in sums {
            by.entry(pass.to_owned()).or_default().push(ms);
        }
    }
    let total: Vec<f64> = frames
        .iter()
        .map(|f| f.iter().map(|(_, ms)| ms).sum())
        .collect();
    serde_json::json!({
        "passes": by.iter().map(|(k, v)| (k.clone(), serde_json::to_value(Spread::of(v)).unwrap_or_default())).collect::<serde_json::Map<_, _>>(),
        "all_passes_ms": Spread::of(&total),
    })
}

#[test]
#[ignore = "G1 export for the path-traced reference; GPU minutes"]
fn g1_export() -> Result<()> {
    let size = size();
    for case in selected()
        .into_iter()
        .filter(|c| c.motion == Motion::Static)
    {
        let dir = out_dir().join(&case.id);
        std::fs::create_dir_all(&dir)?;
        let layout = Layout::new(&case);
        std::fs::write(
            dir.join("scene.json"),
            serde_json::to_vec_pretty(&control::scene_json(&layout, 0, size))?,
        )?;
        let mut bench = Bench::new(layout.clone(), size, entries(&case, size))?;
        bench.pose(0)?;
        let s = settings();
        bench.run_v2(false, Blockers::On, s)?;
        write_f32(&dir.join("v2.f32"), &bench.read(Readback::V2)?)?;
        bench.run_exact(false, Blockers::On, s)?;
        write_f32(&dir.join("exact.f32"), &bench.read(Readback::Exact)?)?;
        if case.target_lpp > 10. {
            bench.run_v2(
                false,
                Blockers::On,
                Settings {
                    omission: Some(0.01),
                    ..s
                },
            )?;
            write_f32(&dir.join("v2-omit.f32"), &bench.read(Readback::V2)?)?;
        }
        bench.destroy()?;
        // Today's renderer: High's four cached local shadows, and none.
        let mut timings = serde_json::Map::new();
        for (name, shadows) in [
            ("control", Some(control::HIGH_LOCAL_SHADOWS)),
            ("control-unshadowed", None),
        ] {
            let run = control::run(&layout, size, 20, 60, shadows)?;
            write_f32(&dir.join(format!("{name}.f32")), &run.radiance)?;
            score::png(
                &dir.join(format!("{name}.png")),
                &run.radiance,
                size,
                layout.ev100,
            )?;
            timings.insert(name.into(), pass_spreads(&run.frames));
        }
        std::fs::write(
            dir.join("control.json"),
            serde_json::to_vec_pretty(&serde_json::Value::Object(timings))?,
        )?;
        eprintln!("exported {}", case.id);
    }
    Ok(())
}

/// Fresh-device verification of dynamic cases: for each sampled frame f
/// (`ARA_LIGHTS_BENCH_FRESH`, default 11,23,35,47), a new bench poses f - 1 then f,
/// running V2 and the exact control in one submission each, and E3 scores that pair
/// (pops between f - 1 and f). The exact control proved history dependent on this
/// device in long runs; on a fresh device every spot check reproduced.
#[test]
#[ignore = "G1 bench: fresh-device E3 verification of the dynamic cases"]
fn g1_fresh() -> Result<()> {
    let size = size();
    let frames: Vec<u32> = std::env::var("ARA_LIGHTS_BENCH_FRESH")
        .unwrap_or_else(|_| "11,23,35,47".into())
        .split(',')
        .filter_map(|f| f.trim().parse().ok())
        .collect();
    let mut all = Vec::new();
    for case in selected()
        .into_iter()
        .filter(|c| c.motion != Motion::Static)
    {
        let dense = case.target_lpp > 10.;
        for omission in [None, Some(0.01)]
            .into_iter()
            .filter(|o| o.is_none() || dense)
        {
            let s = Settings {
                omission,
                ..settings()
            };
            let mut e3 = score::E3::default();
            let (w, h) = (size[0] as usize, size[1] as usize);
            for &f in &frames {
                let mut bench = Bench::new(Layout::new(&case), size, entries(&case, size))?;
                e3.reset_temporal();
                for frame in [f.saturating_sub(1), f] {
                    bench.pose(frame)?;
                    let t = bench.run_pair(false, Blockers::On, s)?;
                    anyhow::ensure!(t.stats.clean(), "{} frame {frame}: fallback", case.id);
                    let test = bench.read(Readback::V2)?;
                    let control = bench.read(Readback::Exact)?;
                    let material = bench.read(Readback::Material)?;
                    bench.run_pair(false, Blockers::Off, s)?;
                    let test_open = bench.read(Readback::V2)?;
                    let control_open = bench.read(Readback::Exact)?;
                    let covered: Vec<bool> =
                        (0..w * h).map(|i| material[i * 4 + 2] > 0.5).collect();
                    let slot = |i: usize| (material[i * 4 + 3] * 255.).round() as u32;
                    let metal: Vec<bool> = (0..w * h)
                        .map(|i| covered[i] && slot(i) == scene::GLOSSY)
                        .collect();
                    let blocker: Vec<bool> = (0..w * h)
                        .map(|i| covered[i] && slot(i) != scene::FLOOR)
                        .collect();
                    e3.add(
                        w,
                        h,
                        &score::Images {
                            test: &test,
                            control: &control,
                            test_open: Some(&test_open),
                            control_open: Some(&control_open),
                            covered: &covered,
                            metal: &metal,
                            blocker: &blocker,
                        },
                    );
                }
                bench.destroy()?;
            }
            let value = serde_json::json!({
                "case": case.id, "omission": omission, "frames": frames,
                "e3_exact": e3.exact(), "highlight_retention": e3.highlight_retention(),
                "shadow_retention": e3.shadow_retention(), "e3": e3,
            });
            eprintln!("{value}");
            all.push(value);
        }
    }
    write("fresh.json", &serde_json::Value::Array(all))
}
