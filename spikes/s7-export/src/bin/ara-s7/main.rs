//! `ara-s7`: the spike's stand-in for the `ara` CLI export surface.
//!
//! - `pack <src> <out> [--profile P]`: cook a content dir into a deterministic `.arapack`.
//! - `pack-info <file>`: verify a pack and print its manifest.
//! - `doctor [--platform P] [--json] [--devices]`: check export prerequisites.
//! - `export --platform P [--lane L] [--profile release] [--json] [--clean]`: run an export lane.
//! - `bench-start <dir> [--runs N] [--wsl]`: cold/warm start timing of an exported folder.
//!
//! `--json` switches output to the NDJSON event stream described in RESULT.md.

mod bench;
mod doctor;
mod env;
mod events;
mod export;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use ara_runtime::pack::{Pack, write};

use crate::events::Events;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let events = Events::new(json);
    match run(&args, &events) {
        Ok(code) => code,
        Err(e) => {
            events.error(
                "ARA-EXP-CLI-001",
                "cli",
                &format!("{e:#}"),
                &[],
                serde_json::json!({}),
            );
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String], events: &Events) -> Result<ExitCode> {
    let (cmd, rest) = args
        .split_first()
        .context("usage: ara-s7 <pack|pack-info|doctor|export|bench-start> ...")?;
    let flag = |name: &str| {
        rest.iter()
            .position(|a| a == name)
            .and_then(|i| rest.get(i + 1))
            .cloned()
    };
    let has = |name: &str| rest.iter().any(|a| a == name);
    let positional: Vec<&String> = {
        let mut out = Vec::new();
        let mut skip = false;
        for a in rest {
            if skip {
                skip = false;
            } else if a.starts_with("--") {
                skip = !matches!(
                    a.as_str(),
                    "--json" | "--devices" | "--clean" | "--wsl" | "--no-verify"
                );
            } else {
                out.push(a);
            }
        }
        out
    };
    match cmd.as_str() {
        "pack" => {
            let [src, out] = positional[..] else {
                bail!("usage: ara-s7 pack <src> <out> [--profile P]")
            };
            let profile = flag("--profile").unwrap_or_else(|| "desktop".into());
            let built = write::build(&PathBuf::from(src), &profile)?;
            std::fs::write(out, &built.bytes).with_context(|| format!("writing {out}"))?;
            events.artifact(
                "pack",
                out,
                built.bytes.len() as u64,
                serde_json::json!({
                    "pack_id": built.id,
                    "file_blake3": blake3::hash(&built.bytes).to_hex().to_string(),
                    "entries": built.entries,
                    "chunks": built.chunks,
                    "raw_bytes": built.raw_bytes,
                }),
            );
            Ok(ExitCode::SUCCESS)
        }
        "pack-info" => {
            let [file] = positional[..] else {
                bail!("usage: ara-s7 pack-info <file>")
            };
            let bytes = std::fs::read(file)?;
            let pack = Pack::open(&bytes)?;
            for path in pack.manifest.entries.keys() {
                pack.read(path)?;
            }
            println!("{}", serde_json::to_string_pretty(&pack.manifest)?);
            println!(
                "pack id {} ({} bytes), all chunks verified",
                pack.id.to_hex(),
                bytes.len()
            );
            Ok(ExitCode::SUCCESS)
        }
        "doctor" => Ok(doctor::run(
            events,
            flag("--platform").as_deref(),
            has("--devices"),
        )),
        "export" => {
            let platform =
                flag("--platform").context("export needs --platform windows|linux|android")?;
            let opts = export::Options {
                platform,
                lane: flag("--lane"),
                profile: flag("--profile").unwrap_or_else(|| "release".into()),
                clean: has("--clean"),
                verify: !has("--no-verify"),
            };
            export::run(events, &opts)
        }
        "bench-start" => {
            let [dir] = positional[..] else {
                bail!("usage: ara-s7 bench-start <dir> [--runs N]")
            };
            let runs = flag("--runs").map(|r| r.parse()).transpose()?.unwrap_or(11);
            bench::start(events, &PathBuf::from(dir), runs)
        }
        other => bail!("unknown command {other}"),
    }
}
