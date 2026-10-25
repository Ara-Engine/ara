//! Immutable service bootstrap and external updater launch.
use anyhow::{Context, Result, ensure};
use ara_update::updater::Plan;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) fn service(directory: &Path, override_path: Option<&Path>) -> Result<PathBuf> {
    let standalone = directory.join("hub.json").is_file();
    // Older updaters forwarded --ara pointing to the previous bootstrap. Such a
    // managed copy must not pin the freshly installed Hub to an obsolete build.
    if let Some(path) = override_path.filter(|path| {
        !standalone
            || !path
                .parent()
                .is_some_and(|p| p.join("service.json").is_file())
    }) {
        return Ok(std::path::absolute(path)?);
    }
    if !standalone {
        return Ok(directory.join(format!("ara{}", std::env::consts::EXE_SUFFIX)));
    }
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is missing")?)
        .join("Ara/Services");
    Ok(ara_update::bootstrap::prepare(
        directory,
        &base,
        env!("CARGO_PKG_VERSION"),
    )?)
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
