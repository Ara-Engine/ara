//! Freeze sample content once through the existing production exporter, never per measurement.
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::binaries::Binaries;

/// Fail early when a sample (`samples/<name>`) lacks fetched art or generated kits.
pub(crate) fn preflight(root: &Path, sample: &str) -> Result<()> {
    let folder = Path::new("samples").join(sample);
    let missing: Vec<_> = crate::git::files(root)?
        .into_iter()
        .filter(|path| path.starts_with(&folder))
        .filter_map(|path| {
            let source = path.file_name()?.to_str()?.strip_suffix(".import.json")?;
            let source = path.with_file_name(source);
            (!root.join(&source).is_file()).then_some(source)
        })
        .collect();
    ensure!(
        missing.is_empty(),
        "{sample} needs fetched art and generated kit assets before its first freeze ({} missing, e.g. {}). Prepare the sample or pass --scene <existing-export-folder>",
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
    pack(
        root,
        base,
        &base.join("outpost"),
        &root.join("samples/outpost"),
        binaries,
    )
}

/// Export `project`'s main scene through the production exporter into `frozen`, once:
/// an existing `frozen/ara-launch.json` is reused. The export cache lives in `base`.
pub(crate) fn pack(
    root: &Path,
    base: &Path,
    frozen: &Path,
    project: &Path,
    binaries: &Binaries,
) -> Result<PathBuf> {
    if frozen.join("ara-launch.json").is_file() {
        return Ok(frozen.to_owned());
    }
    let name = frozen
        .file_name()
        .context("frozen folder name")?
        .to_string_lossy()
        .into_owned();
    let exporter = binaries
        .get("ara-export")
        .context("first perf run needs ara-export (omit --no-build)")?;
    let player = binaries
        // A dev strategy may launch through a wrapper while retaining a clean template.
        .get("ara-player-template")
        .or_else(|| binaries.get("ara-player"))
        .context("missing built ara-player")?;
    let worker = binaries
        .get("ara-import-worker")
        .context("missing built ara-import-worker")?;
    let setup = base.join(format!("freeze-{name}-{}", std::process::id()));
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
            "scriptProject": "", "assembly": assembly_stem(project), "content": "levels",
            "playerTemplate": "runtime", "templates": [pin]
        }))?,
    )?;
    let mut command = Command::new(exporter);
    command.arg("preset-fmt").arg(&preset);
    super::run_command(root, &mut command, "canonical perf export preset", 30)?;
    let candidate = setup.join(&name);
    let mut command = Command::new(exporter);
    command
        .arg("export")
        .arg("--project")
        .arg(project)
        .arg("--preset")
        .arg(&preset)
        .arg("--output")
        .arg(&candidate)
        .arg("--cache")
        .arg(base.join("export-cache"))
        .env("ARA_IMPORT_WORKER", worker)
        .stdout(File::create(setup.join("export.jsonl"))?)
        .stderr(Stdio::inherit());
    eprintln!("Freezing {} once; {}", project.display(), setup.display());
    // Freezing prepares a scene; the measurement that follows carries the guard.
    crate::gpu::locked_unmeasured(3600, || {
        super::run_command(root, &mut command, "sample export", 1800)
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
    if let Some(parent) = frozen.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(dist, frozen)?;
    Ok(frozen.to_owned())
}

/// The shipping C# assembly stem for a frozen sample: its project name reduced to
/// `[A-Za-z0-9_]`, as the exporter requires once a project has script modules
/// (Outpost's C# gameplay); `Game` when the name leaves nothing usable.
fn assembly_stem(project: &Path) -> String {
    let name = fs::read(project.join("ara.project.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|json| json["name"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let stem: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if stem.is_empty() || stem.starts_with(|c: char| c.is_ascii_digit()) {
        "Game".into()
    } else {
        stem
    }
}

#[cfg(test)]
mod assembly_tests {
    use super::*;

    #[test]
    fn assembly_stem_comes_from_the_project_name_and_is_export_safe() {
        let dir = std::env::temp_dir().join(format!("ara-dev-stem-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for (name, stem) in [
            ("Outpost", "Outpost"),
            ("My Game-2", "MyGame2"),
            ("3d", "Game"),
            ("", "Game"),
        ] {
            fs::write(
                dir.join("ara.project.json"),
                serde_json::json!({ "name": name }).to_string(),
            )
            .unwrap();
            assert_eq!(assembly_stem(&dir), stem);
        }
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(assembly_stem(&dir), "Game");
    }
}
