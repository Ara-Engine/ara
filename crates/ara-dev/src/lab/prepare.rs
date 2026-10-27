//! Cancellable cold scene preparation never blocks status or the daemon's other tracks.
use super::{
    session,
    track::{Activity, Track},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

pub(super) struct Pending {
    child: Option<crate::process::Tree>,
    directory: PathBuf,
    candidate: Option<Track>,
}
impl Pending {
    pub fn start(
        root: &Path,
        base: &Path,
        args: &Value,
        binaries: &crate::binaries::Binaries,
    ) -> Result<Self> {
        let directory = base.join("preparations").join(session::id());
        fs::create_dir_all(&directory)?;
        let input = directory.join("request.json");
        session::write(&input, &json!({"arguments":args,"binaries":binaries}))?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.current_dir(root)
            .args(["lab", "prepare-scene"])
            .arg(&input)
            .stdin(Stdio::null())
            .stdout(fs::File::create(directory.join("selection.json"))?)
            .stderr(fs::File::create(directory.join("prepare.log"))?);
        super::build::background(&mut cmd);
        Ok(Self {
            child: Some(crate::process::Tree::spawn(&mut cmd)?),
            directory,
            candidate: None,
        })
    }
    pub fn poll(
        &mut self,
        root: &Path,
        base: &Path,
        executable: &Path,
        name: &str,
        activity: Arc<Activity>,
    ) -> Result<Option<Track>> {
        if let Some(child) = &mut self.child {
            let Some(status) = child.try_wait()? else {
                return Ok(None);
            };
            ensure!(
                status.success(),
                "scene preparation failed: {}; evidence {}",
                fs::read_to_string(self.directory.join("prepare.log")).unwrap_or_default(),
                self.directory.display()
            );
            let selection =
                serde_json::from_slice(&fs::read(self.directory.join("selection.json"))?)?;
            self.child.take();
            super::daemon::vram()?;
            self.candidate = Some(Track::launch(
                root, base, executable, name, selection, activity,
            )?);
        }
        if let Some(candidate) = &mut self.candidate {
            let status = candidate.status()?;
            ensure!(
                status["error"].is_null(),
                "new scene player failed: {}",
                status["error"]
            );
            if status["running"] == true {
                return Ok(self.candidate.take());
            }
        }
        Ok(None)
    }
    pub fn status(&self) -> Value {
        json!({"phase":if self.child.is_some(){"preparing_scene"}else{"warming_player"},"directory":self.directory})
    }
}

pub(super) fn run(root: &Path, input: &Path) -> Result<bool> {
    let _power = super::mac::Job::start()?;
    let base = super::directory(root);
    fs::create_dir_all(base.join("packs"))?;
    let lock = fs::File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(base.join("packs/preparing.lock"))?;
    lock.lock()?;
    let request: Value = serde_json::from_slice(&fs::read(input)?)?;
    let binaries = serde_json::from_value(request["binaries"].clone())
        .context("scene preparation binaries")?;
    let selection = super::scenes::select(root, &base, &request["arguments"], &binaries)?;
    println!("{}", serde_json::to_string(&selection)?);
    Ok(true)
}
