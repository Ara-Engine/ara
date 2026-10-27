//! Incremental build owner and the deliberately small Rust reload strategy seam.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

/// A successful build is staged before the lab touches its last-good processes.
/// Hotpatch integrations can replace this strategy; sessions and transport stay private.
pub(crate) trait ReloadStrategy {
    fn name(&self) -> &'static str;
    fn command(&self, root: &Path, target: &Path) -> Command;
    /// Patch-only builders can return artifacts without producing a new player executable.
    fn stage(&self, directory: &Path, cache: &Path) -> Result<crate::binaries::Binaries> {
        Build::stage_at(directory, cache)
    }
    /// On success, return true for a warm restart. A hotpatch strategy patches the
    /// exact targets and returns false. Errors leave the last good processes alive.
    fn publish(
        &self,
        _binaries: &crate::binaries::Binaries,
        targets: &[ReloadTarget],
    ) -> Result<bool> {
        ensure!(
            targets.iter().all(|t| t.pid > 0 && t.control.is_dir()),
            "reload targets disappeared"
        );
        Ok(true)
    }
}
pub(crate) struct ReloadTarget {
    pub pid: u32,
    pub control: PathBuf,
}
pub(crate) struct WarmRestart;
impl ReloadStrategy for WarmRestart {
    fn name(&self) -> &'static str {
        "rebuild_warm_restart"
    }
    fn command(&self, root: &Path, target: &Path) -> Command {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(root).args([
            "build",
            "--locked",
            "--profile",
            "perf",
            "--message-format=json",
            "--timings",
        ]);
        cmd.args(["--features", "ara-player/fidelity-lab"]);
        for package in [
            "ara-player",
            "ara-shader-cook",
            "ara-export",
            "ara-import-worker",
        ] {
            cmd.args(["-p", package]);
        }
        cmd.env("CARGO_TARGET_DIR", target)
            .env("CARGO_BUILD_BUILD_DIR", target)
            .env("CARGO_BUILD_JOBS", "4")
            .env("CARGO_INCREMENTAL", "1")
            .env("RUSTC_WRAPPER", "");
        background(&mut cmd);
        cmd
    }
}

/// Hidden console children and inherited BelowNormal CPU scheduling, including Cargo workers.
pub(super) fn background(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW,
        };
        command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

pub(super) struct Build {
    pub child: crate::process::Tree,
    pub directory: PathBuf,
    pub started: Instant,
}
impl Build {
    pub fn start(
        root: &Path,
        target: &Path,
        directory: PathBuf,
        strategy: &dyn ReloadStrategy,
    ) -> Result<Self> {
        fs::create_dir_all(&directory)?;
        let mut command = strategy.command(root, target);
        command
            .stdout(fs::File::create(directory.join("cargo.jsonl"))?)
            .stderr(fs::File::create(directory.join("cargo.log"))?);
        let child = crate::process::Tree::spawn(&mut command)?;
        Ok(Self {
            child,
            directory,
            started: Instant::now(),
        })
    }
    pub fn outcome(
        &self,
        success: bool,
        root: &Path,
        claims: &super::claims::Claims,
    ) -> Result<Value> {
        let output = fs::read_to_string(self.directory.join("cargo.jsonl"))?;
        let mut errors = Vec::new();
        for line in output.lines() {
            let Ok(message) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if message["reason"] != "compiler-message" || message["message"]["level"] != "error" {
                continue;
            }
            let spans = message["message"]["spans"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|span| {
                    let path = Path::new(span["file_name"].as_str()?);
                    let path = path.strip_prefix(root).unwrap_or(path);
                    Some(
                        json!({"file":path,"owners":claims.owners(path),"line":span["line_start"]}),
                    )
                })
                .collect::<Vec<_>>();
            errors.push(json!({"message":message["message"]["rendered"],"spans":spans}));
        }
        Ok(
            json!({"success":success,"seconds":self.started.elapsed().as_secs_f64(),"errors":errors,"log":self.directory.join("cargo.log")}),
        )
    }
    fn stage_at(directory: &Path, base: &Path) -> Result<crate::binaries::Binaries> {
        let mut binaries =
            crate::binaries::artifacts(&fs::read_to_string(directory.join("cargo.jsonl"))?)?;
        let player = binaries
            .get_mut("ara-player")
            .context("build did not produce ara-player")?;
        let stage = base.join("players").join(super::session::id());
        fs::create_dir_all(&stage)?;
        let destination = stage.join(player.file_name().context("player filename")?);
        fs::copy(&*player, &destination)?;
        if let Some(parent) = player.parent() {
            let name = "SDL3.dll";
            if parent.join(name).is_file() {
                fs::copy(parent.join(name), stage.join(name))?;
            }
        }
        *player = destination;
        ensure!(player.is_file(), "staged player missing");
        Ok(binaries)
    }
}
