//! `ara-dev d26`: one pass over every D26/D27 release target, written as
//! `target/d26/<timestamp>/report.json` and a self-contained `report.html`.
//!
//! It reuses the existing tools rather than measuring anything itself:
//! - `perf` builds and runs the player on frozen exported packs (Outpost, Hangar,
//!   Showcase) per preset at 2560×1440, validation off, hidden window;
//! - `ara-samples capture --reference` (the visual sweep) supplies ꟻLIP and RMSE;
//! - the editor's `--verify-responsiveness` probe supplies editor startup, Scene CPU,
//!   Play and input latency, on a cold project copy and then warm.
//!
//! Every GPU step runs under the `ara-dev gpu` lock. A row with no measurement reads
//! "not measured", never "pass". Readiness timeouts explicitly fail GPU rows;
//! other missing measurements retain their reason. The report updates per section.
mod html;
mod measure;
mod reclassify;
mod rows;
pub(crate) mod validity;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use serde::Serialize;
use serde_json::json;

use crate::binaries::Binaries;

/// A part of the scoreboard that `--skip` can leave out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Section {
    /// Player GPU/CPU windows and player startup.
    Player,
    /// ꟻLIP/RMSE captures against the path-traced reference.
    Flip,
    /// Editor startup, Scene CPU, Play and responsiveness.
    Editor,
}

#[derive(Args)]
pub(crate) struct Options {
    /// Build profile. D26 names release; perf is the iteration default.
    #[arg(long, default_value = "perf", value_parser = ["perf", "release"])]
    profile: String,
    /// Frames per measured window.
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u32).range(1..))]
    frames: u32,
    /// Frames skipped after renderer readiness (the cold run also retains all CPU frames).
    #[arg(long, default_value_t = 60)]
    warmup: u32,
    /// Fail a GPU row if settled raster content/residency/GI does not arrive in time.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=3600))]
    ready_timeout: u64,
    /// Reuse the previous d26 build of this profile.
    #[arg(long)]
    no_build: bool,
    /// Read ꟻLIP/RMSE from an existing `ara-samples capture` output folder instead of
    /// capturing (`<dir>/<project>/<look>/evidence/<look>-timings.json`).
    #[arg(long)]
    flip_from: Option<PathBuf>,
    /// Path-traced reference samples per pixel for ꟻLIP/RMSE.
    #[arg(long, default_value_t = 256)]
    reference: u32,
    /// Leave a section out; its rows read "not measured".
    #[arg(long, value_enum)]
    skip: Vec<Section>,
    /// Export the frozen packs again even when cached for this sample tree.
    #[arg(long)]
    refreeze: bool,
    /// Build and freeze every scene's pack, then stop without measuring: it can run
    /// outside a quiet window, and the measured run follows with `--no-build`.
    #[arg(long)]
    prepare: bool,
    /// Keep the editor's scratch project copy (deleted by default).
    #[arg(long)]
    keep_project: bool,
    /// Wait for games or foreign GPU work to clear before the three-second baseline.
    #[arg(long)]
    wait_for_quiet: bool,
    #[command(flatten)]
    gpu_policy: crate::gpu_guard::Policy,
    /// Re-score saved GPU evidence and raw source reports without building or launching.
    #[arg(long)]
    reclassify: Option<PathBuf>,
}

/// What the report records about the run itself.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    created_at: String,
    source_head: String,
    source_dirty: bool,
    profile: String,
    resolution: &'static str,
    frames: u32,
    warmup: u32,
    ready_timeout: u64,
    reference_spp: u32,
    build_seconds: f64,
}

pub(crate) fn run(root: &Path, options: Options) -> Result<bool> {
    if let Some(reading) = &options.reclassify {
        return reclassify::run(root, reading, options.gpu_policy);
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let output = root.join("target/d26").join(stamp(now));
    fs::create_dir_all(&output)?;
    eprintln!("D26 scoreboard: {}", output.display());
    let mut results = measure::Results::default();
    for section in &options.skip {
        results
            .skipped
            .insert(*section, "skipped (--skip)".to_owned());
    }
    let (binaries, build_seconds) = match binaries(root, &output, &options) {
        Ok(built) => built,
        Err(error) => {
            let reason = format!("build failed: {error:#}");
            for section in [Section::Player, Section::Flip, Section::Editor] {
                results.skipped.entry(section).or_insert(reason.clone());
            }
            (Binaries::new(), 0.)
        }
    };
    if options.prepare {
        return prepare(root, &output, &binaries, &options, &results);
    }
    let run = Run {
        created_at: stamp(now),
        source_head: crate::git::text(root, ["rev-parse", "HEAD"])?,
        source_dirty: !crate::git::text(root, ["status", "--porcelain"])?.is_empty(),
        profile: options.profile.clone(),
        resolution: "2560x1440",
        frames: options.frames,
        warmup: options.warmup,
        ready_timeout: options.ready_timeout,
        reference_spp: options.reference,
        build_seconds,
    };
    let context = measure::Context {
        root,
        output: &output,
        binaries: &binaries,
        options: &options,
    };
    let load_path = output.join("gpu-load.json");
    let guard = match crate::gpu_guard::Guard::start(
        options.wait_for_quiet,
        load_path.clone(),
        options.gpu_policy,
    ) {
        Ok(guard) => guard,
        Err(error) => {
            let value: serde_json::Value = serde_json::from_slice(&fs::read(&load_path)?)?;
            results.gpu_load = Some(serde_json::from_value(value["reading"].clone())?);
            let verdict = write(&output, &run, &results)?;
            eprintln!("{verdict}; {error:#}");
            println!("Report: {}", output.join("report.html").display());
            return Err(error);
        }
    };
    if guard.snapshot().sampler != "windows_pdh" {
        let mut report = guard.finish()?;
        report
            .errors
            .push("D26 requires native Windows PDH evidence, not a test fixture".into());
        let detail = report.detail();
        results.gpu_load = Some(report);
        write(&output, &run, &results)?;
        return Err(crate::gpu_guard::invalid_error(detail));
    }
    let write = |results: &measure::Results| write(&output, &run, results);
    write(&results)?;
    if !results.skipped.contains_key(&Section::Player) {
        context.player(&mut results);
        results.gpu_load = Some(guard.snapshot());
        write(&results)?;
    }
    if !results.skipped.contains_key(&Section::Flip) {
        context.flip(&mut results);
        results.gpu_load = Some(guard.snapshot());
        write(&results)?;
    }
    if !results.skipped.contains_key(&Section::Editor) {
        context.editor(&mut results);
        results.gpu_load = Some(guard.snapshot());
        write(&results)?;
    }
    results.gpu_load = Some(guard.finish()?);
    let verdict = write(&results)?;
    println!("{verdict}");
    println!("Report: {}", output.join("report.html").display());
    if verdict.contains("INVALID") {
        return Err(crate::gpu_guard::invalid_error(
            results.gpu_load.as_ref().expect("guard report").detail(),
        ));
    }
    Ok(true)
}

/// `--prepare`: freeze each scene's pack with the fresh build and report where the
/// packs are; false when the build or any freeze failed.
fn prepare(
    root: &Path,
    output: &Path,
    binaries: &Binaries,
    options: &Options,
    results: &measure::Results,
) -> Result<bool> {
    if let Some(reason) = results.skipped.get(&Section::Player) {
        eprintln!("D26 prepare: {reason}");
        return Ok(false);
    }
    let context = measure::Context {
        root,
        output,
        binaries,
        options,
    };
    let mut ready = true;
    for scene in &measure::SCENES {
        match context.pack(scene) {
            Ok(pack) => println!("{}: {}", scene.name, pack.display()),
            Err(reason) => {
                println!("{}: freeze failed: {reason}", scene.name);
                ready = false;
            }
        }
    }
    println!(
        "Measure with `ara-dev d26 --no-build --profile {}`",
        options.profile
    );
    Ok(ready)
}

/// Build every root in one feature resolution (the player is the editor's companion
/// too), or reuse the previous build's executables.
fn binaries(root: &Path, output: &Path, options: &Options) -> Result<(Binaries, f64)> {
    let cache = root.join(format!("target/d26/build-paths-{}.json", options.profile));
    if options.no_build {
        let paths: Binaries = serde_json::from_slice(
            &fs::read(&cache).context("--no-build needs a previous d26 build")?,
        )?;
        return Ok((paths, 0.));
    }
    let built = crate::perf::build(
        root,
        output,
        &options.profile,
        &["ara-editor", "ara-samples"],
    )?;
    fs::write(&cache, serde_json::to_vec_pretty(&built.0)?)?;
    Ok(built)
}

/// Write `report.json` and `report.html`; returns the one-line verdict.
fn write(output: &Path, run: &Run, results: &measure::Results) -> Result<String> {
    write_value(output, &serde_json::to_value(run)?, results)
}

fn write_value(
    output: &Path,
    run: &serde_json::Value,
    results: &measure::Results,
) -> Result<String> {
    let mut rows = rows::rows(results);
    let invalid_rows = if let Some(load) = &results.gpu_load {
        validity::apply(&mut rows, load, &results.windows)?
    } else {
        0
    };
    let refused = results
        .gpu_load
        .as_ref()
        .is_some_and(crate::gpu_guard::Report::invalid)
        && rows.iter().all(|row| row.value.is_none());
    let invalid = invalid_rows > 0 || refused;
    let measured = rows
        .iter()
        .filter(|row| {
            row.value.is_some() || matches!(row.status, rows::Status::Invalid | rows::Status::Fail)
        })
        .count();
    let summary = rows::Summary::of(&rows);
    let report = json!({
        "format": "ara.d26/1",
        "run": run,
        "device": results.device,
        "summary": summary,
        "status": if refused||invalid_rows==measured&&invalid {"invalid"} else if invalid {"partially_invalid"}else {"measured"},
        "invalidRows":invalid_rows,
        "gpuLoad": results.gpu_load,
        "diagnostic": if invalid {Some(crate::gpu_guard::CODE)} else {None},
        "skipped": results.skipped,
        "rows": rows,
        "definitions": rows::DEFINITIONS,
    });
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::write(
        output.join("report.html"),
        html::page(&report, &rows, &summary),
    )?;
    Ok(if invalid {
        format!(
            "Reading INVALID ({invalid_rows} overlapping rows): {}",
            summary.verdict()
        )
    } else {
        summary.verdict()
    })
}

/// A folder-safe UTC timestamp, `YYYYMMDD-HHMMSS`.
fn stamp(seconds: u64) -> String {
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let days = (seconds / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let time = seconds % 86_400;
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

/// Copy a project folder, leaving out its caches (`.ara`, `.scratch`).
fn copy_project(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".ara" || name == ".scratch" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Recursively copies `from` into `to` (created if missing). Also seeds the
/// preview checkout's sample cache.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_utc_civil_dates() {
        assert_eq!(stamp(0), "19700101-000000");
        // 2026-10-25 03:04:05 UTC.
        assert_eq!(stamp(1_792_897_445), "20261025-030405");
        // A leap day.
        assert_eq!(stamp(1_709_164_800), "20240229-000000");
    }

    #[test]
    fn unavailable_baseline_refuses_without_invalidating_unrun_rows() {
        let output = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-dev/tests")
            .join(format!("invalid-report-{}", std::process::id()));
        fs::create_dir_all(&output).unwrap();
        let run = Run {
            created_at: "test".into(),
            source_head: "test".into(),
            source_dirty: false,
            profile: "perf".into(),
            resolution: "2560x1440",
            frames: 10,
            warmup: 1,
            ready_timeout: 10,
            reference_spp: 1,
            build_seconds: 0.,
        };
        let results = measure::Results {
            gpu_load: Some(crate::gpu_guard::Report {
                errors: vec!["GPU counter query unavailable".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let verdict = write(&output, &run, &results).unwrap();
        assert!(verdict.contains("INVALID"));
        assert!(!verdict.contains("FAIL"));
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("report.json")).unwrap()).unwrap();
        assert_eq!(report["status"], "invalid");
        assert!(
            report["summary"]["gates"]["D26"]["not_measured"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(
            report["rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["status"] == "not_measured")
        );
        let html = fs::read_to_string(output.join("report.html")).unwrap();
        assert!(html.contains("INVALID"));
        assert!(html.contains("GPU counter query unavailable"));
    }
}
