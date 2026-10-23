//! `ara-s7 bench-start <dist dir>`: start-up and boundary timing of an exported desktop build.
//!
//! Spawns `ara-player --ticks 1 --no-verify` `runs` times with `ARA_T0_UNIX_NS` set just
//! before spawn, so the player can report spawn → first C# call. Run 1 is the coldest the
//! OS file cache allows without admin rights; the rest are warm. A final run measures the
//! Rust↔C# call cost and the 100k-entity tick under the full scene.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::events::Events;

/// Runs the start-up series and the boundary benchmark; emits one `result` event.
pub fn start(ev: &Events, dir: &Path, runs: usize) -> Result<ExitCode> {
    // Absolute: on Unix a relative program path resolves against the child's new cwd.
    let exe = std::path::absolute(dir)?.join(format!("ara-player{}", std::env::consts::EXE_SUFFIX));
    if !exe.is_file() {
        bail!("{} not found; export first", exe.display());
    }
    let mut samples = Vec::new();
    for i in 0..runs {
        let spawned = Instant::now();
        let report = run(&exe, &["--ticks", "1", "--no-verify"])?;
        let wall_ms = spawned.elapsed().as_secs_f64() * 1e3;
        let s = &report["startup"];
        let sample = json!({
            "run": i,
            "spawn_to_first_cs_call_ms": s["spawn_to_first_cs_call_ms"],
            "first_cs_call_ms": s["first_cs_call_ms"],
            "script_load_ms": s["script_load_ms"],
            "process_wall_ms": wall_ms,
        });
        ev.progress("start", &sample.to_string());
        samples.push(sample);
    }
    let field = |k: &str| -> Vec<f64> {
        samples
            .iter()
            .skip(1)
            .filter_map(|s| s[k].as_f64())
            .collect()
    };
    let full = run(&exe, &["--bench", "20000000"])?;
    ev.result(json!({
        "dir": dir,
        "lane": full["lane"],
        "platform": full["platform"],
        "cold": samples.first(),
        "warm_median": {
            "spawn_to_first_cs_call_ms": median(field("spawn_to_first_cs_call_ms")),
            "first_cs_call_ms": median(field("first_cs_call_ms")),
            "script_load_ms": median(field("script_load_ms")),
            "process_wall_ms": median(field("process_wall_ms")),
        },
        "bench": full["bench"],
        "tick": full["tick"],
        "verified": full["verified"],
    }));
    Ok(ExitCode::SUCCESS)
}

fn run(exe: &Path, args: &[&str]) -> Result<Value> {
    let t0 = ara_runtime::runtime::unix_ns().to_string();
    let out = Command::new(exe)
        .current_dir(exe.parent().context("exe has no dir")?)
        .args(args)
        .env("ARA_T0_UNIX_NS", t0)
        .output()
        .with_context(|| format!("spawning {}", exe.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("ARA_RESULT "))
        .with_context(|| {
            format!(
                "no ARA_RESULT from {}:\n{text}{}",
                exe.display(),
                String::from_utf8_lossy(&out.stderr)
            )
        })?;
    Ok(serde_json::from_str(line)?)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}
