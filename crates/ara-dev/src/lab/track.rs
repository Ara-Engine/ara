//! One worker owns each player. The daemon never waits on rendering or player startup.
use super::{
    scenes::Selection,
    session::{self, Session},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    net::TcpStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(super) struct Activity {
    pub gpu: RwLock<()>,
    pub compiling: AtomicBool,
    starts: Mutex<()>,
    pub references: Mutex<()>,
    pub rust_latest: AtomicBool,
    pub shaders_latest: AtomicBool,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            gpu: RwLock::new(()),
            compiling: AtomicBool::new(false),
            starts: Mutex::new(()),
            references: Mutex::new(()),
            rust_latest: AtomicBool::new(false),
            shaders_latest: AtomicBool::new(true),
        }
    }
}
enum Work {
    Control(Value),
    Operation {
        tool: String,
        args: Value,
        reply: TcpStream,
    },
    Restart(PathBuf, Option<std::fs::File>, u64),
}
pub(super) struct Track {
    pub selection: Selection,
    send: mpsc::Sender<Work>,
    status: mpsc::Receiver<Value>,
    latest: Value,
    cancel: Arc<AtomicBool>,
    pub selection_changed: bool,
    resource_view: PathBuf,
    variant: Arc<Mutex<String>>,
    dirty: Arc<AtomicBool>,
    base: PathBuf,
    revision: Arc<AtomicU64>,
    #[cfg(target_os = "macos")]
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Track {
    pub fn launch(
        root: &Path,
        base: &Path,
        executable: &Path,
        name: &str,
        selection: Selection,
        activity: Arc<Activity>,
    ) -> Result<Self> {
        session::name(name)?;
        let initial_generation = super::generations::pin(base, executable)?;
        let cache_base = base.to_owned();
        let (send, jobs) = mpsc::channel();
        let (publish, status) = mpsc::channel();
        let root = root.to_owned();
        let base = base.to_owned();
        let executable = executable.to_owned();
        let name = name.to_owned();
        let chosen = selection.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&cancel);
        let resource_view = super::variants::view(&base, &format!("{name}-{}", session::id()));
        let worker_view = resource_view.clone();
        let variant = Arc::new(Mutex::new(selection.variant.clone()));
        let active_variant = Arc::clone(&variant);
        let dirty = Arc::new(AtomicBool::new(false));
        let pending_shaders = Arc::clone(&dirty);
        let revision = Arc::new(AtomicU64::new(0));
        let requested_revision = Arc::clone(&revision);
        let worker_name = format!("ara-lab-{name}");
        let run = move || {
            let launch = |executable: &Path| -> Result<Session> {
                let _power = super::mac::Job::start()?;
                let _start = activity.starts.lock().expect("player admission");
                #[cfg(not(target_os = "macos"))]
                let _activity = activity.gpu.read().expect("lab activity");
                #[cfg(target_os = "macos")]
                let _activity = activity.gpu.write().expect("lab activity");
                let _visual = crate::gpu::visual(600)?;
                ensure!(
                    !stopped.load(Ordering::Acquire),
                    "track cancelled before player startup"
                );
                super::daemon::vram()?;
                Session::launch(
                    &root,
                    &base,
                    executable,
                    &name,
                    &worker_view,
                    chosen.clone(),
                    Arc::clone(&stopped),
                )
            };
            let mut executable = executable;
            let mut applied_revision = 0;
            let mut initial = launch(&executable);
            drop(initial_generation);
            let mut player = loop {
                match initial {
                    Ok(player) => break player,
                    Err(error) => {
                        let message = format!("{error:#}");
                        let _ = publish.send(json!({"running":false,"error":message}));
                        let (next, generation, revision) = loop {
                            if stopped.load(Ordering::Acquire) {
                                return;
                            }
                            match jobs.recv_timeout(Duration::from_millis(100)) {
                                Ok(Work::Operation { reply, .. }) => {
                                    let _ = super::protocol::reply(
                                        reply,
                                        Err(anyhow::anyhow!("track startup failed: {message}")),
                                    );
                                }
                                Ok(Work::Restart(next, generation, revision)) => {
                                    break (next, generation, revision);
                                }
                                Ok(_) => {}
                                Err(mpsc::RecvTimeoutError::Timeout) => {}
                                Err(_) => return,
                            }
                        };
                        executable = next;
                        initial = launch(&executable);
                        applied_revision = revision;
                        drop(generation);
                    }
                }
            };
            let _ = publish.send(json!({"error":null}));
            let mut up_to_date = true;
            let mut status_at = std::time::Instant::now();
            let _ = publish.send(
                player
                    .status()
                    .unwrap_or_else(|e| json!({"error":e.to_string()})),
            );
            while !stopped.load(Ordering::Acquire) {
                let work = match jobs.recv_timeout(Duration::from_millis(100)) {
                    Ok(work) => work,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if status_at.elapsed() >= Duration::from_millis(250) {
                            let status = player
                                .status()
                                .unwrap_or_else(|e| json!({"running":false,"error":e.to_string()}));
                            let running = status["running"] != false;
                            let _ = publish.send(status);
                            status_at = std::time::Instant::now();
                            if !running {
                                let _ = publish.send(
                                    json!({"error":"player exited; waiting for rebuild or reopen"}),
                                );
                            }
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                let _=publish.send(json!({"busy":true,"pid":player.child.id(),"directory":player.directory,"selection":player.selection}));
                match work {
                    Work::Restart(next, _generation, revision) => {
                        let outcome = (|| -> Result<Session> {
                            let _power = super::mac::Job::start()?;
                            let _start = activity.starts.lock().expect("player admission");
                            #[cfg(not(target_os = "macos"))]
                            let _activity = activity.gpu.read().expect("lab activity");
                            #[cfg(target_os = "macos")]
                            let _activity = activity.gpu.write().expect("lab activity");
                            let _visual = crate::gpu::visual(600)?;
                            super::daemon::vram()?;
                            Session::launch(
                                &root,
                                &base,
                                &next,
                                &name,
                                &worker_view,
                                player.selection.clone(),
                                Arc::clone(&stopped),
                            )
                        })();
                        match outcome {
                            Ok(next_player) => {
                                player = next_player;
                                executable = next;
                                applied_revision = revision;
                                up_to_date = true;
                                let _ = publish.send(json!({"restart_error":null}));
                            }
                            Err(error) => {
                                up_to_date = false;
                                let _=publish.send(json!({"restart_error":format!("{error:#}"),"last_good_retained":true}));
                            }
                        }
                    }
                    Work::Control(request) => {
                        let result = (|| -> Result<Value> {
                            let _activity = activity.gpu.read().expect("lab activity");
                            let _visual = crate::gpu::visual(600)?;
                            pending_shaders.store(false, Ordering::Release);
                            let result = if let Some(name) = request["lab_variant"].as_str() {
                                player.variant(&base, name)
                            } else {
                                let mut request = request;
                                if request["reload_shaders"] == true {
                                    request["frames"] = json!(1);
                                }
                                player.control(request, Duration::from_secs(60))
                            };
                            *active_variant.lock().expect("track variant") =
                                player.selection.variant.clone();
                            result
                        })();
                        if let Err(error) = result {
                            let _ = publish.send(json!({"reload_error":format!("{error:#}")}));
                        }
                    }
                    Work::Operation { tool, args, reply } => {
                        let started = std::time::Instant::now();
                        let result = (|| -> Result<Value> {
                            let _power = super::mac::Job::start()?;
                            let source_current = up_to_date
                                && applied_revision == requested_revision.load(Ordering::Acquire)
                                && activity.rust_latest.load(Ordering::Acquire)
                                && activity.shaders_latest.load(Ordering::Acquire);
                            let mut value = match tool.as_str() {
                                "lab_measure" | "lab_ab" => {
                                    let _exclusive = activity.gpu.write().expect("lab activity");
                                    ensure!(
                                        !activity.compiling.load(Ordering::Acquire),
                                        "wait for the lab's build/cook lane before timing"
                                    );
                                    ensure!(
                                        source_current,
                                        "latest Rust/shader build failed or is pending; the last-good player is retained; inspect lab_status"
                                    );
                                    super::daemon::gpu_preflight()?;
                                    let frames = args["frames"].as_u64().unwrap_or(120);
                                    ensure!((1..=100000).contains(&frames), "invalid frame count");
                                    let preset = args["preset"].as_str().unwrap_or("high");
                                    let policy: crate::gpu_guard::Policy = serde_json::from_value(
                                        args.get("guard").cloned().unwrap_or_else(|| json!({})),
                                    )?;
                                    super::variants::Variants::load(&base)
                                        .ready(&player.selection.variant)?;
                                    player.last_trace = None;
                                    let raw_path = base
                                        .join("measurements")
                                        .join(format!("{}-raw.json", session::id()));
                                    std::fs::create_dir_all(
                                        raw_path.parent().context("measurement parent")?,
                                    )?;
                                    let measured = crate::gpu::locked_with(
                                        600,
                                        !args["no_wait"].as_bool().unwrap_or(false),
                                        policy,
                                        || {
                                            ensure!(
                                                activity.rust_latest.load(Ordering::Acquire)
                                                    && activity
                                                        .shaders_latest
                                                        .load(Ordering::Acquire),
                                                "sources changed while waiting for quiet; retry after publication"
                                            );
                                            if pending_shaders.swap(false, Ordering::AcqRel) {
                                                player.control(
                                                    json!({"reload_shaders":true,"frames":1}),
                                                    Duration::from_secs(60),
                                                )?;
                                            }
                                            let value = if tool == "lab_ab" {
                                                let windows = args["windows"].as_u64().unwrap_or(4);
                                                ensure!(
                                                    (2..=100).contains(&windows),
                                                    "A/B needs 2..100 windows per variant"
                                                );
                                                let experiment = args["b"]
                                                    .as_str()
                                                    .context("experimental variant b required")?;
                                                ensure!(
                                                    args.get("a").is_none_or(|a| a == "baseline"),
                                                    "A/B always compares against baseline A"
                                                );
                                                player.ab(
                                                    &base,
                                                    experiment,
                                                    windows as u32,
                                                    frames as u32,
                                                    preset,
                                                )
                                            } else {
                                                player.measure(frames as u32, preset)
                                            }?;
                                            session::write(
                                                &raw_path,
                                                &json!({"status":"pending_guard","result":value}),
                                            )?;
                                            Ok(value)
                                        },
                                    );
                                    *active_variant.lock().expect("track variant") =
                                        player.selection.variant.clone();
                                    let evidence = crate::gpu::last_reading();
                                    for path in std::iter::once(Some(raw_path.clone()))
                                        .chain(std::iter::once(player.last_trace.clone()))
                                        .flatten()
                                    {
                                        if let Some(mut raw) = session::read(&path) {
                                            raw["status"] = json!(if measured.is_ok() {
                                                "valid_under_recorded_policy"
                                            } else {
                                                "invalid"
                                            });
                                            raw["timing_guard"] = json!(evidence);
                                            session::write(&path, &raw)?;
                                        }
                                    }
                                    match measured {
                                        Ok(mut value) => {
                                            value["timing_guard"] =
                                                json!(crate::gpu::last_reading());
                                            value["raw_report"] = json!(raw_path);
                                            value
                                        }
                                        Err(error) => {
                                            let path = base
                                                .join("measurements")
                                                .join(format!("{}-invalid.json", session::id()));
                                            std::fs::create_dir_all(
                                                path.parent().context("measurement parent")?,
                                            )?;
                                            session::write(
                                                &path,
                                                &json!({"error":format!("{error:#}"),"timing_guard":crate::gpu::last_reading(),"raw_report":raw_path,"ab_trace":player.last_trace}),
                                            )?;
                                            return Err(error.context(format!(
                                                "invalid timing evidence: {}",
                                                path.display()
                                            )));
                                        }
                                    }
                                }
                                "lab_capture" => {
                                    let _activity = activity.gpu.read().expect("lab activity");
                                    let _visual = crate::gpu::visual(600)?;
                                    if pending_shaders.swap(false, Ordering::AcqRel) {
                                        player.control(
                                            json!({"reload_shaders":true,"frames":1}),
                                            Duration::from_secs(60),
                                        )?;
                                    }
                                    let output = base.join("captures").join(session::id());
                                    let mut value = player.capture(&root, &output, &args)?;
                                    let source = super::variants::Variants::load(&base)
                                        .ready(&player.selection.variant);
                                    if let Err(error) = source {
                                        value["last_good_retained"] = json!(true);
                                        value["source_error"] = json!(error.to_string());
                                        value["matched_gate"]["status"] = json!("invalid_source");
                                    }
                                    if !source_current {
                                        value["last_good_retained"] = json!(true);
                                        value["matched_gate"]["status"] = json!("invalid_source");
                                    }
                                    if value["last_good_retained"] == true {
                                        value["quality_gate"]["status"] = json!("invalid_source");
                                    }
                                    value["report"] = json!(output.join("capture.json"));
                                    value
                                }
                                "lab_set" => {
                                    let _activity = activity.gpu.read().expect("lab activity");
                                    let _visual = crate::gpu::visual(600)?;
                                    player.set(
                                        &root,
                                        args.get("values").cloned().unwrap_or_else(|| json!({})),
                                        args["camera"].as_str(),
                                    )?
                                }
                                _ => anyhow::bail!("unknown track operation"),
                            };
                            value["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
                            value["pid"] = json!(player.child.id());
                            value["selection"] = json!(player.selection);
                            value["executable"] = json!(executable);
                            value["source_current_at_start"] = json!(source_current);
                            value["player_revision"] = json!(applied_revision);
                            value["newer_rust_restart_pending"] = json!(
                                applied_revision != requested_revision.load(Ordering::Acquire)
                            );
                            if tool == "lab_capture"
                                && let Some(path) = value["report"].as_str()
                            {
                                session::write(Path::new(path), &value)?;
                            }
                            if tool == "lab_measure" || tool == "lab_ab" {
                                let path = base
                                    .join("measurements")
                                    .join(format!("{}.json", session::id()));
                                std::fs::create_dir_all(
                                    path.parent().context("measurement parent")?,
                                )?;
                                session::write(&path, &value)?;
                                value["report"] = json!(path);
                            }
                            Ok(value)
                        })();
                        let _ = super::protocol::reply(reply, result);
                    }
                }
                let _ = publish.send(
                    player
                        .status()
                        .unwrap_or_else(|e| json!({"error":e.to_string()})),
                );
            }
        };
        let worker = std::thread::Builder::new().name(worker_name).spawn(run)?;
        #[cfg(not(target_os = "macos"))]
        drop(worker);
        Ok(Self {
            selection,
            send,
            status,
            latest: json!({"starting":true}),
            cancel,
            selection_changed: false,
            resource_view,
            variant,
            dirty,
            base: cache_base,
            revision,
            #[cfg(target_os = "macos")]
            worker: Some(worker),
        })
    }
    pub fn status(&mut self) -> Result<Value> {
        while let Ok(update) = self.status.try_recv() {
            if let Some(selection) = update.get("selection") {
                let selection = serde_json::from_value(selection.clone())?;
                self.selection_changed |= self.selection != selection;
                self.selection = selection;
            }
            if let Some(object) = update.as_object() {
                for (key, value) in object {
                    self.latest[key] = value.clone();
                }
            }
            if update.get("running").is_some() {
                self.latest["busy"] = json!(false);
                self.latest["starting"] = json!(false);
            }
        }
        Ok(self.latest.clone())
    }
    pub fn control(&mut self, request: Value, _timeout: Duration) -> Result<Value> {
        self.send.send(Work::Control(request))?;
        Ok(json!({"queued":true}))
    }
    pub fn restart(&self, executable: PathBuf) -> Result<()> {
        let generation = super::generations::pin(&self.base, &executable)?;
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(self
            .send
            .send(Work::Restart(executable, generation, revision))?)
    }
    pub fn operation(&self, tool: String, args: Value, reply: TcpStream) -> Result<()> {
        Ok(self.send.send(Work::Operation { tool, args, reply })?)
    }
    pub fn reload_target(&self) -> Option<super::build::ReloadTarget> {
        if self.latest["running"] != true {
            return None;
        }
        Some(super::build::ReloadTarget {
            pid: self.latest["pid"].as_u64()? as u32,
            control: PathBuf::from(self.latest["directory"].as_str()?).join("control"),
        })
    }
    pub fn can_reuse(&self) -> bool {
        (self.latest["running"] == true || self.latest["starting"] == true)
            && self.latest["error"].is_null()
            && self.latest["restart_error"].is_null()
    }
    pub fn view(&self) -> &Path {
        &self.resource_view
    }
    pub fn active_variant(&self) -> String {
        self.variant.lock().expect("track variant").clone()
    }
    pub fn shaders_changed(&self) {
        self.dirty.store(true, Ordering::Release);
    }
}
impl Drop for Track {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        // Unix child process groups are retired by the worker's owned Session.
        // Let that destructor run before the daemon exits and abandons its threads.
        #[cfg(target_os = "macos")]
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
