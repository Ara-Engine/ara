//! Immutable launcher generations bind the lab's owned PID/control to one dx host.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

pub(super) fn stage(player: &Path, source: &Path) -> Result<PathBuf> {
    let directory = player.parent().context("staged player parent")?;
    let launcher = directory.join(if cfg!(windows) {
        "ara-hotpatch-lab-player.exe"
    } else {
        "ara-hotpatch-lab-player"
    });
    fs::copy(std::env::current_exe()?, &launcher)?;
    fs::write(launcher.with_extension("json"), serde_json::to_vec(source)?)?;
    Ok(launcher)
}

pub(super) fn forward() -> Option<ExitCode> {
    let executable = std::env::current_exe().ok()?;
    if executable.file_stem()? != "ara-hotpatch-lab-player" {
        return None;
    }
    Some(match run(&executable) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("ARA7600: lab hotpatch launcher: {error:#}");
            ExitCode::FAILURE
        }
    })
}

fn run(executable: &Path) -> Result<bool> {
    let manifest: PathBuf = serde_json::from_slice(&fs::read(executable.with_extension("json"))?)?;
    let source = super::source::wait(&manifest)?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let control = args
        .windows(2)
        .find(|a| a[0] == "--perf-control")
        .map(|a| PathBuf::from(&a[1]))
        .context("lab player needs its owned perf-control directory")?;
    ensure!(control.is_dir(), "lab control disappeared");
    let player = fs::read_dir(&source.app_directory)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_stem()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "ara-player" || n.starts_with("ara-player-"))
                && p.extension().is_some_and(|e| e == "exe")
        })
        .context("prepared native player")?;
    let mut command = super::super::tool::command(player);
    command
        .current_dir(&source.root)
        .args(args)
        .env_remove("ARA_HOTPATCH_ARGS")
        .env("ARA_HOTPATCH_LAB_CONTROL", control.join("rust"))
        .env("ARA_HOTPATCH_LAB_OWNER_PID", std::process::id().to_string())
        .env("ARA_HOTPATCH_CONTRACT", source.contract)
        .env("ARA_HOTPATCH_HOST_BIN", "ara-player")
        .env("DIOXUS_BUILD_ID", "0")
        .env("DIOXUS_DEVSERVER_IP", "127.0.0.1")
        .env("DIOXUS_DEVSERVER_PORT", source.port.to_string());
    let result = crate::process::run(&mut command, None)?.success();
    fs::write(source.directory.join("stopped"), b"stopped")?;
    Ok(result)
}
