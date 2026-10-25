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
//! "not measured", never "pass"; a step that fails leaves its rows not measured with
//! the error as the note, and the report is rewritten after every section.
mod html;
mod measure;
mod rows;

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
    /// Frames skipped before a steady window (the cold window skips only the first).
    #[arg(long, default_value_t = 60)]
    warmup: u32,
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
    reference_spp: u32,
    build_seconds: f64,
}

pub(crate) fn run(root: &Path, options: Options) -> Result<bool> {
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
        reference_spp: options.reference,
        build_seconds,
    };
    let context = measure::Context {
        root,
        output: &output,
        binaries: &binaries,
        options: &options,
    };
    let write = |results: &measure::Results| write(&output, &run, results);
    write(&results)?;
    if !results.skipped.contains_key(&Section::Player) {
        context.player(&mut results);
        write(&results)?;
    }
    if !results.skipped.contains_key(&Section::Flip) {
        context.flip(&mut results);
        write(&results)?;
    }
    if !results.skipped.contains_key(&Section::Editor) {
        context.editor(&mut results);
        write(&results)?;
    }
    let verdict = write(&results)?;
    println!("{verdict}");
    println!("Report: {}", output.join("report.html").display());
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
    let rows = rows::rows(results);
    let summary = rows::Summary::of(&rows);
    let report = json!({
        "format": "ara.d26/1",
        "run": run,
        "device": results.device,
        "summary": summary,
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
    Ok(summary.verdict())
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

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
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
}
