//! Incremental build owner and the deliberately small Rust reload strategy seam.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

pub(super) fn default_directory(root: &Path) -> PathBuf {
    std::env::var_os("ARA_LAB_BUILD_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            super::session::read(&super::directory(root).join("build-directory.json"))
                .and_then(|v| v["directory"].as_str().map(PathBuf::from))
        })
        .unwrap_or_else(|| root.join("target"))
}

/// A successful build is staged before the lab touches its last-good processes.
/// Hotpatch integrations can replace this strategy; sessions and transport stay private.
pub(crate) trait ReloadStrategy {
    fn name(&self) -> &'static str;
    /// Complete executables may advance last-good players while newer edits queue.
    /// Patch strategies default to waiting for their source revision to settle.
    fn publish_superseded(&self) -> bool {
        false
    }
    /// Full executable strategies can keep a byte-identical live player.
    fn reuse_unchanged_player(&self) -> bool {
        false
    }
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
    fn reuse_unchanged_player(&self) -> bool {
        true
    }
    fn publish_superseded(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "rebuild_warm_restart"
    }
    fn command(&self, root: &Path, target: &Path) -> Command {
        #[cfg(not(target_os = "macos"))]
        let mut cmd = Command::new("cargo");
        #[cfg(target_os = "macos")]
        let mut cmd = {
            let mut c = Command::new("/usr/bin/nice");
            c.args(["-n", "10", "cargo"]);
            c
        };
        cmd.current_dir(root).args([
            "build",
            "--locked",
            "--profile",
            "perf",
            "--message-format=json",
            "--timings",
        ]);
        cmd.args(["--features", "ara-player/fidelity-lab"]);
        for package in ["ara-player", "ara-export", "ara-import-worker"] {
            cmd.args(["-p", package]);
        }
        #[cfg(not(target_os = "macos"))]
        cmd.args(["-p", "ara-shader-cook"]);
        cmd.env("CARGO_TARGET_DIR", target)
            .env("CARGO_BUILD_BUILD_DIR", target)
            .env("CARGO_BUILD_JOBS", "4")
            .env("CARGO_INCREMENTAL", "1")
            .env("RUSTC_WRAPPER", "");
        #[cfg(target_os = "macos")]
        {
            cmd.env("CARGO_BUILD_JOBS", "1");
        }
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

/// Detach the idle service; only its Cargo/cook children run BelowNormal.
pub(super) fn daemon(command: &mut Command) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::Foundation::{
            HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
        };
        use windows_sys::Win32::System::Threading::{DETACHED_PROCESS, NORMAL_PRIORITY_CLASS};
        for raw in [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ] {
            if !raw.is_null() && raw != INVALID_HANDLE_VALUE {
                // SAFETY: these borrowed handles stay open; only this process's
                // inheritance flags change. Leaking the CLI pipes to a daemon
                // prevents PowerShell/SSH readers from ever observing EOF.
                let changed = unsafe { SetHandleInformation(raw, HANDLE_FLAG_INHERIT, 0) };
                ensure!(
                    changed != 0,
                    "detach standard handle: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
        command.creation_flags(DETACHED_PROCESS | NORMAL_PRIORITY_CLASS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    Ok(())
}

pub(super) struct Build {
    disk_checked: Instant,
    _power: super::mac::Job,
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
        super::mac::build_disk(target)?;
        let power = super::mac::Job::start()?;
        let mut command = strategy.command(root, target);
        command
            .stdout(fs::File::create(directory.join("cargo.jsonl"))?)
            .stderr(fs::File::create(directory.join("cargo.log"))?);
        let child = crate::process::Tree::spawn(&mut command)?;
        Ok(Self {
            disk_checked: Instant::now(),
            _power: power,
            child,
            directory,
            started: Instant::now(),
        })
    }
    pub fn monitor_disk(&mut self, target: &Path) -> Result<()> {
        if self.disk_checked.elapsed().as_secs() >= 5 {
            self.disk_checked = Instant::now();
            super::mac::running_disk(target)?;
        }
        Ok(())
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
        if let Some(cached) = super::session::read(&base.join("binaries.json"))
            .and_then(|v| v["ara-player"].as_str().map(PathBuf::from))
            && same_player(player, &cached)?
        {
            *player = cached;
            return Ok(binaries);
        }
        let stage = base.join("players").join(super::session::id());
        fs::create_dir_all(&stage)?;
        let destination = stage.join(player.file_name().context("player filename")?);
        fs::copy(&*player, &destination)?;
        if let Some(parent) = player.parent() {
            for name in ["SDL3.dll", "libSDL3.dylib"] {
                if parent.join(name).is_file() {
                    fs::copy(parent.join(name), stage.join(name))?;
                }
            }
        }
        *player = destination;
        ensure!(player.is_file(), "staged player missing");
        Ok(binaries)
    }
}

fn same_player(a: &Path, b: &Path) -> Result<bool> {
    use std::io::Read;
    let same = |a: &Path, b: &Path| -> Result<bool> {
        if !a.is_file() || !b.is_file() || fs::metadata(a)?.len() != fs::metadata(b)?.len() {
            return Ok(false);
        }
        let (mut a, mut b) = (fs::File::open(a)?, fs::File::open(b)?);
        let (mut x, mut y) = ([0_u8; 65536], [0_u8; 65536]);
        loop {
            let n = a.read(&mut x)?;
            b.read_exact(&mut y[..n])?;
            if x[..n] != y[..n] {
                return Ok(false);
            }
            if n == 0 {
                return Ok(true);
            }
        }
    };
    if !same(a, b)? {
        return Ok(false);
    }
    for name in ["SDL3.dll", "libSDL3.dylib"] {
        let (a, b) = (a.with_file_name(name), b.with_file_name(name));
        if a.is_file() != b.is_file() || a.is_file() && !same(&a, &b)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod reuse_tests {
    use super::*;
    #[test]
    fn executable_and_dynamic_library_must_both_match() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/lab-tests")
            .join(super::super::session::id());
        let (a, b) = (root.join("a/player"), root.join("b/player"));
        fs::create_dir_all(a.parent().context("parent")?)?;
        fs::create_dir_all(b.parent().context("parent")?)?;
        fs::write(&a, [42_u8; 131073])?;
        fs::copy(&a, &b)?;
        assert!(same_player(&a, &b)?);
        fs::write(a.with_file_name("SDL3.dll"), b"first")?;
        assert!(!same_player(&a, &b)?);
        fs::copy(a.with_file_name("SDL3.dll"), b.with_file_name("SDL3.dll"))?;
        assert!(same_player(&a, &b)?);
        fs::write(b.with_file_name("SDL3.dll"), b"other")?;
        assert!(!same_player(&a, &b)?);
        fs::write(&b, b"different")?;
        assert!(!same_player(&a, &b)?);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
