//! Exact child ownership, correlated controls and persistent track selection.
use super::scenes::Selection;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(super) fn id() -> String {
    format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id()
    )
}
pub(super) fn name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "session/variant names use letters, digits, _ or -"
    );
    Ok(())
}
pub(super) fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
pub(super) fn write(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", id()));
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub(super) struct Session {
    pub selection: Selection,
    pub child: crate::process::Tree,
    pub directory: PathBuf,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Session {
    pub fn launch(
        root: &Path,
        base: &Path,
        executable: &Path,
        name: &str,
        selection: Selection,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self> {
        self::name(name)?;
        let directory = base.join("sessions").join(name).join(id());
        let control = directory.join("control");
        fs::create_dir_all(&control)?;
        fs::create_dir_all(directory.join("empty-layers"))?;
        let mut command = Command::new(executable);
        command
            .current_dir(root)
            .arg("--root")
            .arg(&selection.pack)
            .args([
                "--size",
                &format!("{}x{}", selection.size[0], selection.size[1]),
                "--quality",
                &selection.preset,
                "--perf-frames",
                "1",
                "--perf-warmup",
                "8",
                "--keep-alive",
                "--lab-idle",
                "--perf-wait-ready",
                "--offscreen",
                "--perf-output",
            ])
            .arg(directory.join("player.json"))
            .arg("--perf-control")
            .arg(&control)
            .arg("--gpu-cache")
            .arg(base.join("gpu-cache").join(name))
            .env("ARA_SHADER_DIR", base.join("shaders"))
            .env("ARA_CONFIG_DIR", directory.join("profile"))
            .env("ARA_CRASH_DIR", directory.join("crashes"))
            .env("VK_INSTANCE_LAYERS", "")
            .env("VK_IMPLICIT_LAYER_PATH", directory.join("empty-layers"))
            .env("SDL_AUDIODRIVER", "dummy")
            .env("SDL_MAC_BACKGROUND_APP", "1")
            .stdin(Stdio::null())
            .stdout(fs::File::create(directory.join("player.log"))?)
            .stderr(fs::File::create(directory.join("player.stderr.log"))?);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, NORMAL_PRIORITY_CLASS};
            command.creation_flags(CREATE_NO_WINDOW | NORMAL_PRIORITY_CLASS);
        }
        let child = crate::process::Tree::spawn(&mut command)?;
        let mut session = Self {
            selection,
            child,
            directory,
            cancel,
        };
        session.control(
            json!({"camera":session.selection.camera,"measure":false}),
            Duration::from_secs(180),
        )?;
        let preset = session.selection.preset.clone();
        session.measure(1, &preset)?;
        Ok(session)
    }
    pub fn status(&mut self) -> Result<Value> {
        Ok(
            json!({"pid":self.child.id(),"running":self.child.try_wait()?.is_none(),"selection":self.selection,"live":read(&self.directory.join("status.json")),"directory":self.directory}),
        )
    }
    pub fn control(&mut self, mut request: Value, timeout: Duration) -> Result<Value> {
        let id = id();
        request["request_id"] = json!(id);
        write(
            &self.directory.join("control").join(format!("{id}.json")),
            &request,
        )?;
        let deadline = Instant::now() + timeout;
        let response = self.directory.join("control").join(format!("{id}.reply"));
        loop {
            ensure!(
                !self.cancel.load(std::sync::atomic::Ordering::Acquire),
                "lab track stopped"
            );
            if let Some(value) = read(&response) {
                if let Some(error) = value["error"].as_str() {
                    bail!("{error}");
                }
                return Ok(value);
            }
            ensure!(
                self.child.try_wait()?.is_none(),
                "warm player exited; {}",
                self.directory.join("player.stderr.log").display()
            );
            ensure!(
                Instant::now() < deadline,
                "player control timed out: {}",
                self.directory.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn measure(&mut self, frames: u32, preset: &str) -> Result<Value> {
        ensure!((1..=100000).contains(&frames), "invalid frame count");
        ensure!(
            ["low", "medium", "high", "epic"].contains(&preset),
            "unknown quality preset"
        );
        let result = self.control(
            json!({"measure":true,"frames":frames,"quality":preset}),
            Duration::from_secs(180),
        )?;
        self.selection.preset = preset.into();
        Ok(result)
    }
    pub fn capture(&mut self, root: &Path, output: &Path) -> Result<Value> {
        fs::create_dir_all(output)?;
        let shots = super::scenes::shots(root, &self.selection.scene)?;
        let reference = std::env::var_os("ARA_LAB_REFERENCES")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("D:/AraBench/report/images"));
        let quality: Value = if self.selection.scene == "showcase" {
            serde_json::from_str(include_str!("quality/showcase.json"))?
        } else {
            json!({"regions":[]})
        };
        self.control(json!({"capture":{"directory":output,"shots":shots,"size":self.selection.size,"references":reference.join(&self.selection.scene).join("cycles"),"quality":quality}}), Duration::from_secs(180))
    }
    pub fn executable(binaries: &crate::binaries::Binaries) -> Result<&Path> {
        Ok(binaries
            .get("ara-player")
            .context("initial lab build is still running (or failed); inspect lab_status")?)
    }
}
