//! Immutable service bootstrap and external updater launch.
use anyhow::{Context, Result, ensure};
use ara_update::updater::Plan;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) fn service(directory: &Path) -> Result<PathBuf> {
    if !directory.join("hub.json").is_file() {
        return Ok(directory.join(format!("ara{}", std::env::consts::EXE_SUFFIX)));
    }
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is missing")?)
        .join("Ara/Services");
    fs::create_dir_all(&base)?;
    let target = base.join(env!("CARGO_PKG_VERSION"));
    if !target.exists() {
        let staged = base.join(format!(".service-{}", std::process::id()));
        fs::create_dir(&staged)?;
        for entry in fs::read_dir(directory.join("bootstrap"))? {
            let entry = entry?;
            ensure!(
                entry.file_type()?.is_file(),
                "service bootstrap must contain only files"
            );
            fs::copy(entry.path(), staged.join(entry.file_name()))?;
        }
        ara_update::write_json(
            &staged.join("service.json"),
            &serde_json::json!({"version":env!("CARGO_PKG_VERSION")}),
        )?;
        fs::rename(staged, &target)?;
    }
    let executable = target.join("ara.exe");
    ensure!(
        executable.is_file(),
        "Hub bootstrap service is missing; reinstall Ara Hub"
    );
    Ok(executable)
}
pub(super) fn launch(
    config: &Path,
    ara: &Path,
    archive: PathBuf,
    manifest: String,
    signature: String,
) -> Result<()> {
    let directory = std::env::current_exe()?
        .parent()
        .context("Hub executable directory")?
        .to_path_buf();
    ensure!(
        directory.join("hub.json").is_file() && !directory.join("engine.json").exists(),
        "Install the standalone Ara Hub to enable self-updates; engine-bundled Hubs are updated with their engine"
    );
    fs::create_dir_all(config.join("updater"))?;
    let runner = config
        .join("updater")
        .join(format!("ara-hub-updater-{}.exe", std::process::id()));
    fs::copy(directory.join("ara-hub-updater.exe"), &runner)?;
    let plan = Plan {
        current: directory,
        archive,
        manifest,
        signature,
        config: std::path::absolute(config)?,
        ara: ara.to_path_buf(),
    };
    let request = config
        .join("updater")
        .join(format!("plan-{}.json", std::process::id()));
    ara_update::write_json(&request, &plan)?;
    let mut command = Command::new(runner);
    command
        .env("ARA_CONFIG_DIR", config)
        .arg(&request)
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(config.join("hub-update.log"))?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().context("start external Hub updater")?;
    Ok(())
}
