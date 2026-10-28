//! Opt-in desktop integration probe using a real pinned player and authored project:
//! a Windows export, or on a Mac a macOS `.app` (pass the player's dSYM DWARF file as
//! `--symbols`).
use anyhow::{Context, Result};
use ara_export::{ExportPreset, ExportRequest};
use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    project: PathBuf,
    #[arg(long)]
    template: PathBuf,
    #[arg(long)]
    symbols: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    cache: PathBuf,
}

fn pin(path: &Path) -> Result<serde_json::Value> {
    let source = std::path::absolute(path)?;
    Ok(
        serde_json::json!({"hash":blake3::hash(&std::fs::read(&source)?).to_string(),
        "source":source.to_str().context("artifact path is not UTF-8")?}),
    )
}

fn main() -> Result<()> {
    let args = Args::parse();
    let (platform, arch) = if cfg!(target_os = "macos") {
        ("macos", "arm64")
    } else {
        ("windows", "x64")
    };
    let mut template = pin(&args.template)?;
    template["platform"] = platform.into();
    template["arch"] = arch.into();
    template["config"] = "dev".into();
    template["symbols"] = pin(&args.symbols)?;
    let authored = ara_host::project::authored_root(&args.project);
    let content = authored
        .strip_prefix(&args.project)?
        .to_string_lossy()
        .replace('\\', "/");
    let content = if content.is_empty() { "." } else { &content };
    let preset: ExportPreset = serde_json::from_value(serde_json::json!({
        "platform":platform,"config":"dev","lane":"nativeAot",
        "playerTemplate":"runtime","scriptProject":"","assembly":"AraGame",
        "content":content,"templates":[template]
    }))?;
    preset.validate()?;
    let mut events = ara_export::event::Ndjson(std::io::stdout().lock());
    let summary = ara_export::export(
        &ExportRequest {
            project: args.project,
            preset,
            output: args.output,
            cache: args.cache,
            device: None,
            allow_emulator: false,
        },
        &mut events,
    )?;
    anyhow::ensure!(
        summary.verify.runtime_executed && summary.verify.screenshot.is_some(),
        "export must execute the authored world and render an image"
    );
    Ok(())
}
