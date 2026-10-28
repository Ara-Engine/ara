//! Manual latency experiments, confined to the optional driver and one named crate.
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, ffi::OsString};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    crates: BTreeMap<String, Compiler>,
    link_threads: Option<u32>,
    link_debug: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Compiler {
    codegen_units: Option<u32>,
    incremental: Option<bool>,
}

fn read() -> Result<Settings> {
    std::env::var_os("ARA_HOTPATCH_EXPERIMENT")
        .map(|path| Ok(serde_json::from_slice(&std::fs::read(path)?)?))
        .unwrap_or_else(|| Ok(Settings::default()))
}

pub(super) fn compiler(crate_name: &str, args: &mut Vec<OsString>) -> Result<()> {
    let settings = read()?;
    let Some(settings) = settings.crates.get(crate_name) else {
        return Ok(());
    };
    if let Some(units) = settings.codegen_units {
        ensure!(
            (1..=1024).contains(&units),
            "codegen_units must be 1..=1024"
        );
        args.push(format!("-Ccodegen-units={units}").into());
    }
    if settings.incremental == Some(false) {
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-C"
                && args
                    .get(i + 1)
                    .is_some_and(|a| a.to_string_lossy().starts_with("incremental="))
            {
                args.drain(i..i + 2);
            } else if args[i].to_string_lossy().starts_with("-Cincremental=") {
                args.remove(i);
            } else {
                i += 1;
            }
        }
    }
    Ok(())
}

pub(super) fn linker(command: &mut std::process::Command) -> Result<()> {
    let settings = read()?;
    if let Some(threads) = settings.link_threads {
        ensure!((1..=64).contains(&threads), "link_threads must be 1..=64");
        command.arg(format!("/threads:{threads}"));
    }
    if settings.link_debug == Some(false) {
        anyhow::bail!(
            "ARA7600: Windows dx needs the patch PDB to construct its jump table; /DEBUG:NONE requires rebuilding with debug symbols restored"
        );
    }
    Ok(())
}
