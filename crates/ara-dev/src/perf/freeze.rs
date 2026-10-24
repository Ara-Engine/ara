//! Freeze sample content once through the existing production exporter, never per measurement.
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::binaries::Binaries;

pub(super) fn preflight(root: &Path) -> Result<()> {
    let missing: Vec<_> = crate::git::files(root)?
        .into_iter()
        .filter(|path| path.starts_with("samples/outpost"))
        .filter_map(|path| {
            let source = path.file_name()?.to_str()?.strip_suffix(".import.json")?;
            let source = path.with_file_name(source);
            (!root.join(&source).is_file()).then_some(source)
        })
        .collect();
    ensure!(
        missing.is_empty(),
        "Outpost needs fetched art and generated kit assets before its first freeze ({} missing, e.g. {}). Prepare the sample or pass --scene <existing-export-folder>",
        missing.len(),
        missing
            .iter()
            .take(3)
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

pub(super) fn outpost(root: &Path, base: &Path, binaries: &Binaries) -> Result<PathBuf> {
    let frozen = base.join("outpost");
    if frozen.join("ara-launch.json").is_file() {
        return Ok(frozen);
    }
    let exporter = binaries
        .get("ara-export")
        .context("first perf run needs ara-export (omit --no-build)")?;
    let player = binaries
        .get("ara-player")
        .context("missing built ara-player")?;
    let worker = binaries
        .get("ara-import-worker")
        .context("missing built ara-import-worker")?;
    let setup = base.join(format!("freeze-{}", std::process::id()));
    fs::create_dir_all(&setup)?;
    let template = setup.join("runtime.zip");
    ensure!(
        !template.exists(),
        "previous freeze attempt exists: {}",
        setup.display()
    );
    let mut command = Command::new(exporter);
    command
        .arg("runtime-template")
        .arg("--player")
        .arg(player)
        .arg("--engine")
        .arg(root)
        .arg("--output")
        .arg(&template)
        .stdout(File::create(setup.join("template.log"))?)
        .stderr(Stdio::inherit());
    super::run_command(root, &mut command, "runtime template", 180)?;
    let pin = setup.join("pin.json");
    let pdb = player.with_file_name(format!(
        "{}.pdb",
        player
            .file_stem()
            .context("player name")?
            .to_string_lossy()
            .replace('-', "_")
    ));
    ensure!(
        pdb.is_file(),
        "initial freeze requires player symbols; build the perf profile first or pass --scene <existing-export-folder>"
    );
    let mut command = Command::new(exporter);
    command
        .args([
            "pin",
            "--platform",
            "windows",
            "--arch",
            "x64",
            "--config",
            "profile",
            "--file",
        ])
        .arg(&template)
        .arg("--symbols")
        .arg(&pdb)
        .stdout(File::create(&pin)?)
        .stderr(Stdio::inherit());
    super::run_command(root, &mut command, "runtime pin", 60)?;
    let pin: serde_json::Value = serde_json::from_slice(&fs::read(pin)?)?;
    let preset = setup.join("perf.export.json");
    fs::write(
        &preset,
        serde_json::to_vec_pretty(&serde_json::json!({
            "$schema": "ara://schema/export/1", "platform": "windows", "config": "profile",
            "scriptProject": "", "assembly": "", "content": "levels", "playerTemplate": "runtime", "templates": [pin]
        }))?,
    )?;
    let mut command = Command::new(exporter);
    command.arg("preset-fmt").arg(&preset);
    super::run_command(root, &mut command, "canonical perf export preset", 30)?;
    let candidate = setup.join("outpost");
    let mut command = Command::new(exporter);
    command
        .arg("export")
        .arg("--project")
        .arg(root.join("samples/outpost"))
        .arg("--preset")
        .arg(&preset)
        .arg("--output")
        .arg(&candidate)
        .arg("--cache")
        .arg(base.join("export-cache"))
        .env("ARA_IMPORT_WORKER", worker)
        .stdout(File::create(setup.join("export.jsonl"))?)
        .stderr(Stdio::inherit());
    eprintln!("Freezing the authored Outpost once; {}", setup.display());
    crate::gpu::locked(3600, || {
        super::run_command(root, &mut command, "Outpost export", 1800)
    })?;
    // Export publishes a dist folder; accept only the actual runtime launch contract.
    let dist = if candidate.join("ara-launch.json").is_file() {
        candidate
    } else {
        candidate.join("build")
    };
    ensure!(
        dist.join("ara-launch.json").is_file(),
        "export did not publish ara-launch.json"
    );
    fs::rename(dist, &frozen)?;
    Ok(frozen)
}
