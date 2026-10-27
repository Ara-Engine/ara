//! Desktop runtime template (A6 `ara-player`). Prebuilt once per (platform, arch, config);
//! an export only drops a pack, the C# image and `ara-launch.json` next to it.
//!
//! Usage: `ara-player [--root DIR] [--ticks N] [--bench N] [--no-verify] [--mod PATH]`.
//! `ARA_T0_UNIX_NS` (set by `ara-s7 bench-start`) enables spawn → first-C#-call timing.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use ara_export_player::log;
use ara_export_player::runtime::{RunConfig, ScriptLane, run};
use serde::Deserialize;

/// `ara-launch.json`, written by the exporter next to the template.
#[derive(Deserialize)]
struct Launch {
    #[serde(default)]
    contract: String,
    /// Pack file, relative to the root.
    pack: String,
    /// `nativeaot` or `r2r-coreclr`.
    lane: String,
    /// NativeAOT image file, or the CoreCLR folder, relative to the root.
    script: String,
    /// Assembly name for the CoreCLR lane.
    assembly: Option<String>,
    #[serde(default)]
    config: String,
    #[serde(default)]
    encryption_key_env: Option<String>,
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
    if ara_crash::monitor_from_args().map_err(|e| e.to_string())? {
        return Ok(true);
    }
    let mut root = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .map(PathBuf::from)
        .ok_or("exe has no parent dir")?;
    let mut ticks = None;
    let mut bench_iters = 0;
    let mut verify = true;
    let mut mod_path = None;
    let mut crash_probe = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--headless" => {}
            "--root" => root = PathBuf::from(value()?),
            "--ticks" => ticks = Some(value()?.parse().map_err(|e| format!("--ticks: {e}"))?),
            "--bench" => bench_iters = value()?.parse().map_err(|e| format!("--bench: {e}"))?,
            "--no-verify" => verify = false,
            "--mod" => mod_path = Some(PathBuf::from(value()?)),
            "--crash-probe" => crash_probe = Some(value()?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if root.join("data/ara-launch.json").is_file() {
        root = root.join("data");
    }
    let _crash_capture = ara_export_player::shipping::start(&root)?;
    let crash_probe = match crash_probe.as_deref() {
        Some("native") => Some(ara_export_player::shipping::CrashProbe::Native),
        Some("panic") => Some(ara_export_player::shipping::CrashProbe::Panic),
        Some("managed") => Some(ara_export_player::shipping::CrashProbe::Managed),
        Some(_) => return Err("--crash-probe expects native, panic or managed".into()),
        None => None,
    };
    let launch_text = std::fs::read(root.join("ara-launch.json"))
        .map_err(|e| format!("reading ara-launch.json: {e}"))?;
    let launch: Launch =
        serde_json::from_slice(&launch_text).map_err(|e| format!("ara-launch.json: {e}"))?;
    if !launch.contract.is_empty() && launch.contract != "ara.player.conformance/1" {
        return Err("ARA-EXP-TPL-005: this is the conformance template; runtime contract requires an integrated engine player pin".into());
    }
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
        crash_probe,
        pack_key: launch
            .encryption_key_env
            .as_ref()
            .map(|name| {
                let text = std::env::var(name).map_err(|_| {
                    "ARA-EXP-PACK-002: runtime key environment variable is unset".to_string()
                })?;
                ara_pack::EncryptionKey::from_hex(&text).map_err(|e| e.to_string())
            })
            .transpose()?,
        lane,
        ticks,
        bench_iters,
        verify,
        mod_path,
        spawn_unix_ns,
    };
    #[cfg(feature = "dev-bridge")]
    let bridge = ara_export_player::observability::start(&launch.config)?;
    #[cfg(not(feature = "dev-bridge"))]
    let _ = &launch.config;
    let report = run(&cfg, &pack, t0)?;
    #[cfg(feature = "dev-bridge")]
    if let Some(bridge) = &bridge {
        ara_export_player::observability::report(bridge, &report);
        let wait = std::env::var("ARA_BRIDGE_WAIT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
            .min(30000);
        std::thread::sleep(std::time::Duration::from_millis(wait));
    }
    let ok = report.verified && report.fault_contained;
    println!(
        "ARA_RESULT {}",
        serde_json::to_string(&report).map_err(|e| e.to_string())?
    );
    Ok(ok)
}
