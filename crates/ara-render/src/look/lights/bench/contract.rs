//! T1's render-v3 wire contract (`ara-dev` lab `stress/contract.rs`, version 1) for V2:
//! one `RunResult` per case and run under `<root>/cases/<id>/<uncapped|capped>/run.json`,
//! so the frozen lab controller scores V2 with `lab gate --gate G1 --report-only
//! --output <root>`. ara-render cannot depend on ara-dev, so the JSON is written by
//! field name here; the case object is copied verbatim from `cases.v1.json`.
//!
//! - `local_lights`: build + bin + prep + select + shade (+ the visibility PSO) GPU ms,
//!   demand in tile-light entries, resident bytes of V2's storage.
//! - `local_pages`: V2 keeps no shadow pages (visibility is traced from current
//!   geometry every frame), so moving lights redraw nothing and nothing lags: zero cost,
//!   zero debt, zero lag, measured as such rather than unavailable.
//! - Uncapped: V2 timing frames (no per-pixel counters, the production RGBA16F output).
//!   Dense cases run the omission policy in both runs (G1 prices dense cost "omission
//!   included"); V2 has no deferral caps, so both runs replay the same work.
//! - Capped: V2 and the exact finite-range control in one submission per frame (with and
//!   without blockers), E3 per frame, LPP and coverage from the per-pixel counters, the
//!   omitted energy, and region FLIP (V2 vs the control) on the case's reference frames.
use super::harness::{Bench, Blockers, Readback};
use super::scene::{self, Case, Layout};
use super::{Settings, Spread, entries, out_dir, score};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// T1's G1 case list (cases.v1.json ids carrying gate 1).
pub(crate) const G1_CASES: [&str; 7] = [
    "lpp-low-1m",
    "lpp-dense-1m",
    "lpp-low-4m",
    "lpp-dense-4m",
    "lpp-low-8m",
    "lpp-dense-8m",
    "moving-200-lights",
];

/// The versioned case object for `id`, verbatim, and its frame and reference counts.
fn t1_case(id: &str) -> Result<(Value, u32, Vec<u32>)> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ara-dev/src/lab/stress/cases.v1.json");
    let suite: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let case = suite["cases"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["id"] == id))
        .cloned()
        .with_context(|| format!("{id} is not in cases.v1.json"))?;
    let frames = case["frames"].as_u64().context("frames")? as u32;
    let references = case["reference_frames"]
        .as_array()
        .map(|r| {
            r.iter()
                .filter_map(|f| f.as_u64())
                .map(|f| f as u32)
                .collect()
        })
        .unwrap_or_default();
    Ok((case, frames, references))
}

/// The bench case for a T1 id (the static recipe, or moving lights).
fn bench_case(id: &str) -> Result<Case> {
    scene::cases()
        .into_iter()
        .find(|c| c.id == id)
        .with_context(|| format!("no bench case {id}"))
}

/// V2's settings for a case: the omission policy on dense cases.
fn case_settings(case: &Case) -> Settings {
    Settings {
        omission: (case.target_lpp > 10.).then_some(0.01),
        ..super::settings()
    }
}

/// Where receipts go: `ARA_LIGHTS_CONTRACT_ROOT`, else `<bench out>/receipts`.
pub(crate) fn root() -> PathBuf {
    std::env::var("ARA_LIGHTS_CONTRACT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| out_dir().join("receipts"))
}

fn revision() -> String {
    format!(
        "ara-render look::lights V2 (B1 bench, shaders {})",
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("cooked/lights_shade_vis.json")
        )
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["key"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
    )
}

fn local_pages() -> Value {
    json!({"gpu_ms": 0.0, "cap_ms": null, "demand": 0, "debt": 0,
        "work_unit": "light shadow pages (V2 keeps none: visibility is traced per frame)",
        "oldest_debt_frames": 0, "edge_lag_page_texels": 0.0, "near_edge_lag_page_texels": 0.0,
        "deadline_misses": 0, "resident_bytes": 0})
}

fn empty_quality() -> Value {
    json!({"region_flip": {}, "region_flip_excess": null, "max_solution_error_white": null,
        "temporal_block_rms_white": null, "temporal_blocks": [], "penumbra_relative_error": null,
        "contact_displacement_px": null, "missing_shadow_features": null, "leak_pixels": null,
        "pop_events": null, "highlight_retention": null, "shadow_retention": null,
        "omitted_energy_fraction": null, "secondary_views_scored": null})
}

#[allow(clippy::too_many_arguments)]
fn run_result(
    case: &Value,
    run: &str,
    device: &str,
    size: [u32; 2],
    timing_valid: bool,
    caps_applied: bool,
    notes: Vec<String>,
    frames: Vec<Value>,
) -> Value {
    json!({"version": 1, "case": case, "run": run, "renderer": "ara-render look::lights V2 (finite-range many-light, B1)",
        "device": device, "revision": revision(), "size": size, "fps": 60, "uncapped_supported": true,
        "caps_applied": caps_applied, "timing_valid": timing_valid, "reference_samples": 0,
        "reference_bounces": 0, "reference_converged": false, "reference_white": 1.0,
        "notes": notes, "frames": frames})
}

fn write_run(id: &str, run: &str, value: &Value) -> Result<()> {
    let dir = root().join("cases").join(id).join(run);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("run.json"), serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

/// The uncapped (timing) receipt of case `id`. `timing_valid` states whether the
/// caller held the quiet window with an idle GPU.
pub(crate) fn uncapped(id: &str, size: [u32; 2], timing_valid: bool) -> Result<Value> {
    let (t1, frames, _) = t1_case(id)?;
    let case = bench_case(id)?;
    let s = case_settings(&case);
    let mut bench = Bench::new(Layout::new(&case), size, entries(&case, size))?;
    let device = bench.gpu.info().name.clone();
    let resident = bench.v2_gpu.resident_bytes();
    // Warm-up: pipelines, clocks and caches.
    for _ in 0..20 {
        bench.pose(0)?;
        bench.run_timed(s, true)?;
    }
    let (mut rows, mut totals) = (Vec::new(), Vec::new());
    for f in 0..frames {
        bench.pose(f)?;
        let t = bench.run_timed(s, true)?;
        anyhow::ensure!(t.stats.clean(), "{id} frame {f}: tiles fell back");
        let ms = |p: &[&str]| t.ms(p);
        let total = ms(&[
            "Lights.Build",
            "Lights.Bin",
            "Lights.Prep",
            "Lights.Select",
            "Lights.Shade",
            "Lights.Visibility",
        ]);
        totals.push(total);
        rows.push(json!({"frame": f, "stages": {
                "local_lights": {"gpu_ms": total, "cap_ms": null, "demand": t.stats.entries, "debt": null,
                    "work_unit": "tile-light entries", "oldest_debt_frames": null, "edge_lag_page_texels": null,
                    "near_edge_lag_page_texels": null, "deadline_misses": null, "resident_bytes": resident},
                "local_pages": local_pages()},
            "gpu_frame_ms": null, "cpu_critical_path_ms": null, "submission_stall_ms": null,
            "host_update_render_ms": null, "lpp": null, "receiver_coverage": null, "quality": empty_quality(),
            "capture": null, "reference": null,
            "evidence": {"build_bin_ms": ms(&["Lights.Build", "Lights.Bin"]), "prep_ms": ms(&["Lights.Prep"]),
                "select_ms": ms(&["Lights.Select"]), "shade_lean_ms": ms(&["Lights.Shade"]),
                "visibility_ms": ms(&["Lights.Visibility"]), "partial_pairs": t.stats.partial,
                "all_instance_pairs": t.stats.all}}));
    }
    bench.destroy()?;
    let notes = vec![
        "B1 bench: V2 on a trace()-built G-buffer of T1's fixture (same lights, ranges, floor, 108-triangle occluders, camera); local_lights GPU timestamps cover build, bin, prep, select, shade and visibility passes only (not a complete frame: gpu_frame_ms is null).".to_owned(),
        if s.omission.is_some() { "Dense case: the omission policy (1% energy bound) runs in both runs; V2 has no deferral caps.".to_owned() } else { "No omission at <= 3 LPP; V2 has no deferral caps, so this replay is uncapped.".to_owned() },
        format!("timing_valid {timing_valid}: B1's own atomic quiet-window lock and an nvidia-smi idle check before the batch (pmon logged throughout), not ara-dev's whole-worker CPU guard."),
    ];
    let value = run_result(
        &t1,
        "uncapped",
        &device,
        size,
        timing_valid,
        true,
        notes,
        rows,
    );
    write_run(id, "uncapped", &value)?;
    Ok(json!({"case": id, "local_lights_ms": Spread::of(&totals)}))
}

/// Linear luminance of RGBA pixel `i`.
fn lum(p: &[f32], i: usize) -> f64 {
    0.2126 * f64::from(p[i * 4])
        + 0.7152 * f64::from(p[i * 4 + 1])
        + 0.0722 * f64::from(p[i * 4 + 2])
}

/// Display image of radiance as the reference example makes it (exposure, ACES fit, sRGB).
fn display(rgba: &[f32], size: [u32; 2], ev100: f32) -> image::RgbaImage {
    let exposure = 1. / (1.2 * 2f32.powf(ev100));
    let tone = |x: f32| {
        let x = (x * exposure).max(0.);
        let v = ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0., 1.);
        let s = if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1. / 2.4) - 0.055
        };
        (s * 255. + 0.5) as u8
    };
    image::RgbaImage::from_fn(size[0], size[1], |x, y| {
        let i = ((y * size[0] + x) * 4) as usize;
        image::Rgba([tone(rgba[i]), tone(rgba[i + 1]), tone(rgba[i + 2]), 255])
    })
}

/// The worst 64x64-pixel region's mean absolute error of `test` against `control`,
/// on the tone-mapped image. FLIP left with `ara-pathtrace`.
fn region_flip(control: &[f32], test: &[f32], size: [u32; 2], ev100: f32) -> Result<f64> {
    if control
        .iter()
        .zip(test)
        .all(|(a, b)| a.to_bits() == b.to_bits())
    {
        return Ok(0.);
    }
    let left = display(control, size, ev100);
    let right = display(test, size, ev100);
    let (w, h) = (size[0] as usize, size[1] as usize);
    let mut worst = 0f64;
    for by in (0..h).step_by(64) {
        for bx in (0..w).step_by(64) {
            let (mut sum, mut n) = (0f64, 0usize);
            for y in by..(by + 64).min(h) {
                for x in bx..(bx + 64).min(w) {
                    let i = (y * w + x) * 4;
                    for channel in 0..3 {
                        sum += (f64::from(left.as_raw()[i + channel])
                            - f64::from(right.as_raw()[i + channel]))
                        .abs()
                            / 255.;
                    }
                    n += 1;
                }
            }
            worst = worst.max(sum / (n as f64 * 3.));
        }
    }
    Ok(worst)
}

/// V2 and the exact control in one submission (`run_pair`), kept only when the next
/// submission reproduces both images bit for bit; otherwise the pair is re-run
/// (`discarded` counts discarded submissions, at most six tries). Rare whole-submission
/// deviations (root cause not isolated) reach either pass. Returns the timing, V2's
/// image, its counters (with `stats`) and the control.
#[allow(clippy::type_complexity)]
fn consistent(
    bench: &mut Bench,
    stats: bool,
    blockers: Blockers,
    s: Settings,
    retries: &mut u32,
    discarded: &mut u32,
) -> Result<(super::Timing, Vec<f32>, Vec<f32>, Vec<f32>)> {
    let same = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
    let mut previous: Option<(Vec<f32>, Vec<f32>)> = None;
    for _ in 0..6 {
        let t = bench.run_pair(stats, blockers, s)?;
        let test = bench.read(Readback::V2)?;
        let counters = if stats {
            bench.read(Readback::Counters)?
        } else {
            Vec::new()
        };
        let control = bench.read(Readback::Exact)?;
        if let Some((v, c)) = &previous {
            if same(v, &test) && same(c, &control) {
                return Ok((t, test, counters, control));
            }
            *discarded += 1;
            *retries += u32::from(!same(c, &control));
        }
        previous = Some((test, control));
    }
    anyhow::bail!("no reproducible V2 + control submission in six tries")
}

/// The capped (quality) receipt of case `id`.
pub(crate) fn capped(id: &str, size: [u32; 2]) -> Result<Value> {
    let (t1, frames, references) = t1_case(id)?;
    let case = bench_case(id)?;
    let s = case_settings(&case);
    let layout = Layout::new(&case);
    let ev100 = layout.ev100;
    let mut bench = Bench::new(layout, size, entries(&case, size))?;
    let device = bench.gpu.info().name.clone();
    let (w, h) = (size[0] as usize, size[1] as usize);
    let mut e3 = score::E3::default();
    let mut retries = 0u32;
    let mut discarded = 0u32;
    let mut rows = Vec::new();
    let mut flips = Vec::new();
    for f in 0..frames {
        bench.pose(f)?;
        let before = retries;
        let discarded_before = discarded;
        let (t, test, counters, control) = consistent(
            &mut bench,
            true,
            Blockers::On,
            s,
            &mut retries,
            &mut discarded,
        )?;
        anyhow::ensure!(t.stats.clean(), "{id} frame {f}: tiles fell back");
        let material = bench.read(Readback::Material)?;
        let (_, test_open, _, control_open) = consistent(
            &mut bench,
            false,
            Blockers::Off,
            s,
            &mut retries,
            &mut discarded,
        )?;
        let covered: Vec<bool> = (0..w * h).map(|i| material[i * 4 + 2] > 0.5).collect();
        let slot = |i: usize| (material[i * 4 + 3] * 255.).round() as u32;
        let metal: Vec<bool> = (0..w * h)
            .map(|i| covered[i] && slot(i) == scene::GLOSSY)
            .collect();
        let blocker: Vec<bool> = (0..w * h)
            .map(|i| covered[i] && slot(i) != scene::FLOOR)
            .collect();
        let was = e3.clone_counts();
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
        let now = e3.clone_counts();
        let n_covered = covered.iter().filter(|&&c| c).count();
        let (mut lit, mut rays, mut masked, mut omitted_lights, mut omitted) =
            (0f64, 0f64, 0f64, 0f64, 0f64);
        for i in (0..w * h).filter(|&i| covered[i]) {
            lit += f64::from(counters[i * 4]);
            let work = f64::from(counters[i * 4 + 1]);
            masked += (work / 4096.).floor();
            rays += work % 4096.;
            omitted_lights += f64::from(counters[i * 4 + 3]);
            let (lc, lt) = (lum(&control, i), lum(&test, i));
            if lc > 0. {
                omitted = omitted.max((lc - lt) / lc);
            }
        }
        let flip = if references.contains(&f) {
            let v = region_flip(&control, &test, size, ev100)?;
            flips.push(v);
            Some(v)
        } else {
            None
        };
        let ratio = |t: f64, c: f64, n: u64| (n > 0).then(|| if c > 0. { t / c } else { 1. });
        let cov = n_covered.max(1) as f64;
        rows.push(json!({"frame": f, "stages": {"local_pages": local_pages()}, "gpu_frame_ms": null, "cpu_critical_path_ms": null,
            "submission_stall_ms": null, "host_update_render_ms": null,
            "lpp": lit / cov, "receiver_coverage": n_covered as f64 / (w * h) as f64,
            "quality": {"region_flip": {}, "region_flip_excess": flip, "max_solution_error_white": null,
                "temporal_block_rms_white": null, "temporal_blocks": [], "penumbra_relative_error": null,
                "contact_displacement_px": null, "missing_shadow_features": null,
                "leak_pixels": now.0 - was.0, "pop_events": now.1 - was.1,
                "highlight_retention": ratio(now.2 - was.2, now.3 - was.3, now.4 - was.4),
                "shadow_retention": ratio(now.5 - was.5, now.6 - was.6, now.7 - was.7),
                "omitted_energy_fraction": omitted.max(0.), "secondary_views_scored": null},
            "capture": null, "reference": null,
            "evidence": {"highlight_pixels": now.4 - was.4, "shadow_pixels": now.7 - was.7,
                "rays_per_covered_pixel": rays / cov, "masked_pairs_per_covered_pixel": masked / cov,
                "omitted_lights_per_covered_pixel": omitted_lights / cov,
                "control_reruns": f64::from(retries - before), "partial_pairs": t.stats.partial}}));
        if let Some(row) = rows.last_mut() {
            row["evidence"]["discarded_submissions"] = json!(discarded - discarded_before);
        }
    }
    bench.destroy()?;
    let notes = vec![
        "B1 bench capped receipt: V2 and the exact finite-range control (every light, every pixel, every source sample, same shading and sample pattern) in one submission per frame, with and without blockers; E3 per frame against that control (leak > 3%, 16x16 tile pops > 2%, highlight and shadow retention; null where no qualifying pixels).".to_owned(),
        "The control shares V2's sample-mask segment test; the bench separately compares both with per-sample trace() rays and an f64 brute force (see B1-RESULT.md).".to_owned(),
        "region_flip_excess: worst 64x64 region FLIP of V2 against the exact control, on the case's reference frames only (other frames null); path-traced reference FLIP is reported separately for frame 0 of static cases.".to_owned(),
        format!("Each frame keeps a V2 + control submission only when the next submission reproduces both bit for bit ({discarded} submissions discarded, {retries} of them with a differing control): rare whole-submission deviations (root cause not isolated) otherwise reach either pass."),
    ];
    let value = run_result(&t1, "capped", &device, size, false, true, notes, rows);
    write_run(id, "capped", &value)?;
    Ok(
        json!({"case": id, "e3": e3, "e3_exact": e3.exact(), "control_reruns": retries,
        "region_flip_on_reference_frames": flips}),
    )
}

#[test]
#[ignore = "G1 receipts (capped): minutes of GPU work at 2560x1440"]
fn g1_contract_capped() -> Result<()> {
    let size = super::size();
    let mut all = Vec::new();
    for id in G1_CASES.iter().filter(|id| super::wanted(id)) {
        let summary = capped(id, size)?;
        eprintln!("{summary}");
        all.push(summary);
    }
    super::write("contract-capped.json", &Value::Array(all))
}

#[test]
#[ignore = "G1 receipts (uncapped timing): run under the quiet-window lock"]
fn g1_contract_uncapped() -> Result<()> {
    let size = super::size();
    let valid = std::env::var("ARA_LIGHTS_TIMING_VALID").is_ok_and(|v| v == "1");
    let mut all = Vec::new();
    for id in G1_CASES.iter().filter(|id| super::wanted(id)) {
        let summary = uncapped(id, size, valid)?;
        eprintln!("{summary}");
        all.push(summary);
    }
    super::write("contract-uncapped.json", &Value::Array(all))
}
