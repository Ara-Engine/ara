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
    pub last_trace: Option<PathBuf>,
    root: PathBuf,
    shader_view: PathBuf,
    // A control acknowledgement precedes the asynchronous status-file refresh.
    shader_cohorts: Option<Value>,
    _generation: Option<fs::File>,
}
impl Session {
    pub fn launch(
        root: &Path,
        base: &Path,
        executable: &Path,
        name: &str,
        view: &Path,
        selection: Selection,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self> {
        self::name(name)?;
        #[cfg(target_os = "macos")]
        super::mac::scripts(root, base, &selection.pack, &selection.scene)?;
        let generation = super::generations::pin(base, executable)?;
        let directory = base.join("sessions").join(name).join(id());
        let control = directory.join("control");
        fs::create_dir_all(&control)?;
        fs::create_dir_all(directory.join("empty-layers"))?;
        let mut command = Command::new(executable);
        super::variants::publish_tree(&super::variants::profile(base, &selection.variant), view)?;
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
            .env("ARA_SHADER_DIR", view)
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
            last_trace: None,
            root: root.to_owned(),
            shader_view: view.to_owned(),
            shader_cohorts: None,
            _generation: generation,
        };
        session.control(
            json!({"camera":session.selection.camera,"params":session.selection.params,"measure":false}),
            Duration::from_secs(180),
        )?;
        let preset = session.selection.preset.clone();
        session.measure_window(1, &preset)?;
        Ok(session)
    }
    pub fn status(&mut self) -> Result<Value> {
        Ok(
            json!({"pid":self.child.id(),"running":self.child.try_wait()?.is_none(),"selection":self.selection,"live":read(&self.directory.join("status.json")),"directory":self.directory}),
        )
    }
    pub fn control(&mut self, mut request: Value, timeout: Duration) -> Result<Value> {
        let _power = super::mac::Job::start()?;
        let id = id();
        request["request_id"] = json!(id);
        write(
            &self.directory.join("control").join(format!("{id}.json")),
            &request,
        )?;
        let deadline = Instant::now() + timeout;
        let response = self.directory.join("control").join(format!("{id}.reply"));
        loop {
            if self.cancel.load(std::sync::atomic::Ordering::Acquire) {
                self.child.stop();
                bail!("lab track stopped");
            }
            if let Some(value) = read(&response) {
                if let Some(error) = value["error"].as_str() {
                    bail!("{error}");
                }
                if let Some(cohorts) = [
                    &value["shaders"]["cohorts"],
                    &value["player"]["shaders"]["cohorts"],
                ]
                .into_iter()
                .find(|cohorts| cohorts.is_object())
                {
                    self.shader_cohorts = Some(cohorts.clone());
                }
                return Ok(value);
            }
            ensure!(
                self.child.try_wait()?.is_none(),
                "warm player exited; {}",
                self.directory.join("player.stderr.log").display()
            );
            if Instant::now() >= deadline {
                self.child.stop();
                bail!(
                    "player control timed out; stopped owned player {} before releasing the GPU lease: {}",
                    self.child.id(),
                    self.directory.display()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn measure_window(&mut self, frames: u32, preset: &str) -> Result<Value> {
        ensure!((1..=100000).contains(&frames), "invalid frame count");
        ensure!(
            ["low", "medium", "high", "epic"].contains(&preset),
            "unknown quality preset"
        );
        let result = self.control(
            json!({"measure":true,"frames":frames,"quality":preset}),
            Duration::from_secs(180),
        )?;
        crate::perf::readiness::validate(&result["player"]).map_err(anyhow::Error::msg)?;
        self.selection.preset = preset.into();
        Ok(result)
    }
    pub fn measure(&mut self, frames: u32, preset: &str) -> Result<Value> {
        let mut measured = self.measure_window(frames, preset)?;
        measured["temporal"] = self.temporal()?["temporal"].clone();
        Ok(measured)
    }
    pub fn temporal(&mut self) -> Result<Value> {
        use std::hash::{Hash, Hasher};
        let base = self.directory.ancestors().nth(3).context("lab base")?;
        let quality = self.quality()?;
        let status = read(&self.directory.join("status.json")).context("player status")?;
        let cohorts = self
            .shader_cohorts
            .as_ref()
            .unwrap_or(&status["shaders"]["cohorts"]);
        let identity = json!({"format":"ara.lab.temporal-cache/3","generation":read(&base.join("generation.json")),"selection":self.selection,"shaders":cohorts,"content_generation":status["readiness"]["generation"],"config":quality["temporal"]});
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        identity.to_string().hash(&mut hasher);
        let cache = base
            .join("measurements/temporal-cache")
            .join(format!("{:016x}.json", hasher.finish()));
        if let Some(mut value) = read(&cache)
            && ["static", "pan"].iter().all(|mode| {
                [
                    &value["temporal"][mode]["blocks"],
                    &value["temporal"][mode]["frame_flip"]["blocks"],
                ]
                .iter()
                .all(|p| p.as_str().is_some_and(|p| Path::new(p).is_file()))
            })
        {
            value["temporal"]["cache"]["status"] = json!("reused");
            return Ok(value);
        }
        let directory = base.join("measurements").join(format!("{}-temporal", id()));
        let mut value=self.control(json!({"temporal":{"directory":directory,"config":quality["temporal"]},"measure":false}),Duration::from_secs(180))?;
        value["temporal"]["cache"] = json!({"status":"fresh","identity":identity,"report":cache,"computed_unix_ms":crate::d26::validity::unix_ms()?});
        fs::create_dir_all(cache.parent().context("temporal cache parent")?)?;
        write(&cache, &value)?;
        Ok(value)
    }
    pub fn capture(&mut self, root: &Path, output: &Path, options: &Value) -> Result<Value> {
        fs::create_dir_all(output)?;
        let shots = super::scenes::shots(root, &self.selection.scene)?;
        let reference = std::env::var_os("ARA_LAB_REFERENCES")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("D:/AraBench/report/images"));
        let mut quality = self.quality()?;
        quality["capture_profile"] = json!(format!(
            "{}@{}x{}",
            self.selection.preset, self.selection.size[0], self.selection.size[1]
        ));
        if self.selection.scene != "showcase" {
            quality["regions"] = json!([]);
        }
        let linear = std::env::var_os("ARA_LAB_LINEAR_REFERENCES")
            .map(PathBuf::from)
            .unwrap_or_else(|| super::directory(root).join("references"));
        let linear = linear.join(&self.selection.scene).join("cycles");
        let policy = read(&linear.join("linear-reference.json")).unwrap_or(Value::Null);
        if let Some(scale) = policy["radiance_scale"].as_f64() {
            quality["reference_radiance_scale"] = json!(scale);
        }
        self.control(json!({"capture":{"directory":output,"shots":shots,"size":self.selection.size,"references":reference.join(&self.selection.scene).join("cycles"),"linear_references":linear,"linear_policy":policy,"quality":quality,"runs":options["runs"].as_u64().unwrap_or(3),"temporal":options["temporal"].as_bool().unwrap_or(true)}}), Duration::from_secs(300))
    }
    fn quality(&self) -> Result<Value> {
        Ok(serde_json::from_slice(&fs::read(
            self.root
                .join("crates/ara-dev/src/lab/quality/showcase.json"),
        )?)?)
    }
    pub fn variant(&mut self, base: &Path, name: &str) -> Result<Value> {
        self::name(name)?;
        super::variants::Variants::load(base).ready(name)?;
        let view = self.shader_view.clone();
        let previous = self.selection.variant.clone();
        super::variants::publish_selection(&super::variants::profile(base, name), &view)?;
        match self.control(
            json!({"reload_shaders":true,"measure":false,"frames":1}),
            Duration::from_secs(60),
        ) {
            Ok(result) => {
                self.selection.variant = name.into();
                Ok(json!({"variant":name,"reload":result}))
            }
            Err(error) => {
                super::variants::publish_selection(
                    &super::variants::profile(base, &previous),
                    &view,
                )?;
                let _ = self.control(
                    json!({"reload_shaders":true,"measure":false,"frames":1}),
                    Duration::from_secs(60),
                );
                Err(error)
            }
        }
    }
    pub fn set(&mut self, root: &Path, changes: Value, camera: Option<&str>) -> Result<Value> {
        let entries = changes
            .as_object()
            .context("parameter values must be an object")?;
        let camera = camera
            .map(|name| {
                super::scenes::shots(root, &self.selection.scene)?
                    .into_iter()
                    .find(|s| s["name"] == name)
                    .map(|s| s["camera"].clone())
                    .with_context(|| format!("unknown camera {name}"))
            })
            .transpose()?;
        let result = self.control(
            json!({"params":changes,"camera":camera,"measure":false}),
            Duration::from_secs(60),
        )?;
        ensure!(
            result.get("params").is_some(),
            "this last-good player predates live parameters; wait for the current lab build"
        );
        if camera.is_some() {
            self.selection.camera = camera;
        }
        let selection = self
            .selection
            .params
            .as_object_mut()
            .context("parameter selection")?;
        for (key, value) in entries {
            selection.insert(key.clone(), value.clone());
        }
        Ok(result)
    }
    pub fn executable(binaries: &crate::binaries::Binaries) -> Result<&Path> {
        Ok(binaries
            .get("ara-player")
            .context("initial lab build is still running (or failed); inspect lab_status")?)
    }
}
