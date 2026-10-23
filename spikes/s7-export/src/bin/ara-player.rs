//! Desktop runtime template (A6 `ara-player`). Prebuilt once per (platform, arch, config);
//! an export only drops a pack, the C# image and `ara-launch.json` next to it.
//!
//! Usage: `ara-player [--root DIR] [--ticks N] [--bench N] [--no-verify] [--mod PATH]`.
//! `ARA_T0_UNIX_NS` (set by `ara-s7 bench-start`) enables spawn → first-C#-call timing.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use ara_runtime::log;
use ara_runtime::runtime::{RunConfig, ScriptLane, run};
use serde::Deserialize;

/// `ara-launch.json`, written by the exporter next to the template.
#[derive(Deserialize)]
struct Launch {
    /// Pack file, relative to the root.
    pack: String,
    /// `nativeaot` or `r2r-coreclr`.
    lane: String,
    /// NativeAOT image file, or the CoreCLR folder, relative to the root.
    script: String,
    /// Assembly name for the CoreCLR lane.
    assembly: Option<String>,
}

fn main() -> ExitCode {
    let t0 = Instant::now();
    match real_main(t0) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            log::write(2, "rt", &format!("fatal: {e}"));
            ExitCode::from(2)
        }
    }
}

fn real_main(t0: Instant) -> Result<bool, String> {
    let mut root = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .map(PathBuf::from)
        .ok_or("exe has no parent dir")?;
    let mut ticks = None;
    let mut bench_iters = 0;
    let mut verify = true;
    let mut mod_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--root" => root = PathBuf::from(value()?),
            "--ticks" => ticks = Some(value()?.parse().map_err(|e| format!("--ticks: {e}"))?),
            "--bench" => bench_iters = value()?.parse().map_err(|e| format!("--bench: {e}"))?,
            "--no-verify" => verify = false,
            "--mod" => mod_path = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let launch_text = std::fs::read(root.join("ara-launch.json"))
        .map_err(|e| format!("reading ara-launch.json: {e}"))?;
    let launch: Launch =
        serde_json::from_slice(&launch_text).map_err(|e| format!("ara-launch.json: {e}"))?;
    let lane = match launch.lane.as_str() {
        "nativeaot" => ScriptLane::Aot {
            lib: root.join(&launch.script),
        },
        "r2r-coreclr" => ScriptLane::Clr {
            dir: root.join(&launch.script),
            assembly: launch.assembly.unwrap_or_else(|| "Game".into()),
        },
        other => return Err(format!("unknown lane {other}")),
    };
    let spawn_unix_ns = std::env::var("ARA_T0_UNIX_NS")
        .ok()
        .and_then(|s| s.parse().ok());
    let pack = std::fs::read(root.join(&launch.pack))
        .map_err(|e| format!("reading {}: {e}", launch.pack))?;
    let cfg = RunConfig {
        lane,
        ticks,
        bench_iters,
        verify,
        mod_path,
        spawn_unix_ns,
    };
    let report = run(&cfg, &pack, t0)?;
    let ok = report.verified && report.fault_contained;
    println!(
        "ARA_RESULT {}",
        serde_json::to_string(&report).map_err(|e| e.to_string())?
    );
    Ok(ok)
}
