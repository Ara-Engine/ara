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
    e3_exact: bool,
    highlight_retention: f64,
    shadow_retention: f64,
    /// Lights reaching each covered pixel (positive shading, before visibility).
    lpp_covered: f64,
    /// Over the whole screen.
    lpp_screen: f64,
    /// Segments traced per covered pixel.
    rays_per_pixel: f64,
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
    let (mut lit, mut rays, mut covered_px, mut omitted) = (0f64, 0f64, 0f64, 0f64);
    let mut max_omitted = 0f64;
    let mut stats = Vec::new();
    let dir = out_dir().join(&case.id);
    let shadows = true;
    for f in 0..frames {
        bench.pose(f)?;
        let timing = bench.run_v2(true, Blockers::On, settings)?;
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
        bench.run_exact(false, Blockers::On, settings)?;
        let control = bench.read(Readback::Exact)?;
        let (test_open, control_open) = if shadows {
            bench.run_v2(false, Blockers::Off, settings)?;
            let t = bench.read(Readback::V2)?;
            bench.run_exact(false, Blockers::Off, settings)?;
            (Some(t), Some(bench.read(Readback::Exact)?))
        } else {
            (None, None)
        };
        let covered: Vec<bool> = (0..w * h).map(|i| material[i * 4 + 2] > 0.5).collect();
        let slot = |i: usize| material[i * 4 + 3].round() as u32;
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
                test_open: test_open.as_deref(),
                control_open: control_open.as_deref(),
                covered: &covered,
                metal: &metal,
                blocker: &blocker,
            },
        );
        for i in (0..w * h).filter(|&i| covered[i]) {
            lit += f64::from(counters[i * 4]);
            rays += f64::from(counters[i * 4 + 1]);
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
                |i| counters[i * 4 + 1],
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
        highlight_retention: e3.highlight_retention(),
        shadow_retention: e3.shadow_retention(),
        e3,
        lpp_covered: lit / covered_px.max(1.),
        lpp_screen: lit / (f64::from(frames) * (w * h) as f64),
        rays_per_pixel: rays / covered_px.max(1.),
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
) -> Result<serde_json::Value> {
    let layout = Layout::new(case);
    let mut bench = Bench::new(layout, size, entries(case, size))?;
    let mut series: [Vec<f64>; 6] = Default::default();
    let mut first = None;
    for f in 0..warm + frames {
        let frame = if case.motion == Motion::Static {
            0
        } else {
            f % 300
        };
        bench.pose(frame)?;
        let t = bench.run_v2(false, Blockers::On, settings)?;
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
        let shade = t.ms(&["Lights.Shade"]);
        for (s, v) in series.iter_mut().zip([
            build,
            prep,
            select,
            shade,
            build + prep + select + shade,
            t.ms(&["Bench.Primary"]),
        ]) {
            s.push(v);
        }
    }
    bench.destroy()?;
    let [build, prep, select, shade, total, primary] = series.map(|s| Spread::of(&s));
    Ok(serde_json::json!({
        "case": case.id, "size": size, "samples": settings.samples, "omission": settings.omission,
        "frames": frames, "warm": warm,
        "build_ms": build, "prep_ms": prep, "select_ms": select, "shade_ms": shade, "total_ms": total,
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
    for case in selected() {
        let mut s = settings();
        all.push(timing(&case, size, 20, frames, s)?);
        eprintln!("{}", all.last().map(|v| v.to_string()).unwrap_or_default());
        if case.target_lpp > 10. {
            s.omission = Some(0.01);
            all.push(timing(&case, size, 20, frames, s)?);
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
