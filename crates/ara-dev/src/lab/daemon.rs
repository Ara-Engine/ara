//! Checkout singleton. Shader cooking and Rust compilation have independent bounded lanes.
use super::{
    build::{Build, ReloadStrategy, WarmRestart},
    claims::Claims,
    protocol::{Endpoint, Request},
    session,
    track::{Activity, Track as Session},
    watch::Watch,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File, OpenOptions},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

struct Cook {
    child: crate::process::Tree,
    manifest: PathBuf,
    log: PathBuf,
    started: Instant,
    output: PathBuf,
    previous: String,
}
struct Lab {
    root: PathBuf,
    base: PathBuf,
    target: PathBuf,
    max_sessions: u32,
    binaries: crate::binaries::Binaries,
    build: Option<Build>,
    rebuild: bool,
    last_build: Value,
    cook: Option<Cook>,
    cooks: BTreeSet<PathBuf>,
    shader_results: BTreeMap<PathBuf, Value>,
    watch: Watch,
    claims: Claims,
    sessions: BTreeMap<String, Session>,
    events: VecDeque<Value>,
    stopping: bool,
    strategy: Box<dyn ReloadStrategy>,
    activity: std::sync::Arc<Activity>,
}
impl Lab {
    fn event(&mut self, kind: &str, value: Value) {
        let event = json!({"at":crate::d26::validity::unix_ms().unwrap_or_default(),"kind":kind,"detail":value});
        eprintln!("{event}");
        self.events.push_back(event);
        if self.events.len() > 1000 {
            self.events.pop_front();
        }
    }
    fn persist(&self) -> Result<()> {
        session::write(&self.base.join("claims.json"), &self.claims)?;
        let selections: BTreeMap<_, _> = self
            .sessions
            .iter()
            .map(|(name, s)| (name, &s.selection))
            .collect();
        session::write(&self.base.join("selections.json"), &selections)
    }
    fn status(&mut self) -> Result<Value> {
        let sessions = self
            .sessions
            .iter_mut()
            .map(|(name, s)| Ok((name.clone(), s.status()?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        Ok(
            json!({"format":"ara.lab/1","pid":std::process::id(),"root":self.root,"build_dir":self.target,"strategy":self.strategy.name(),"building":self.build.is_some(),"rebuild_queued":self.rebuild,"last_build":self.last_build,"shader_packages":self.watch.manifests().len(),"shader_queue":self.cooks,"shader_results":self.shader_results,"claims":self.claims,"sessions":sessions,"max_sessions":self.max_sessions}),
        )
    }
    fn tick(&mut self) -> Result<()> {
        let mut selection_changed = false;
        for session in self.sessions.values_mut() {
            session.status()?;
            selection_changed |= std::mem::take(&mut session.selection_changed);
        }
        if selection_changed {
            self.persist()?;
        }
        self.activity.compiling.store(
            self.build.is_some() || self.cook.is_some() || !self.cooks.is_empty() || self.rebuild,
            std::sync::atomic::Ordering::Release,
        );
        let changes = self.watch.poll(&self.root)?;
        for path in &changes {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ["rs", "toml", "lock"].contains(&ext) {
                self.rebuild = true;
            }
            for manifest in self.watch.manifests() {
                if path.starts_with(manifest.parent().context("shader source parent")?) {
                    self.cooks.insert(manifest);
                }
            }
        }
        if !changes.is_empty() {
            self.event("source_changes", json!(changes));
        }
        if let Some(build) = &mut self.build
            && let Some(status) = build.child.try_wait()?
        {
            let build = self.build.take().context("completed build")?;
            self.last_build = build.outcome(status.success(), &self.root, &self.claims)?;
            if status.success() && self.rebuild {
                self.last_build["superseded"] = json!(true);
                self.event("newer_sources_queued", self.last_build.clone());
            } else if status.success() {
                match self
                    .strategy
                    .stage(&build.directory, &self.base)
                    .and_then(|binaries| self.adopt(binaries))
                {
                    Ok(()) => self.event("build_published", self.last_build.clone()),
                    Err(error) => {
                        self.last_build["restart_error"] = json!(format!("{error:#}"));
                        self.event("last_good_retained", self.last_build.clone());
                    }
                }
            } else {
                self.event("compile_failed_last_good_retained", self.last_build.clone());
            }
        }
        let activity = std::sync::Arc::clone(&self.activity);
        let admission = activity.gpu.try_read();
        if self.rebuild && self.build.is_none() && admission.is_ok() {
            self.rebuild = false;
            self.build = Some(Build::start(
                &self.root,
                &self.target,
                self.base.join("builds").join(session::id()),
                self.strategy.as_ref(),
            )?);
            self.event(
                "build_started",
                json!({"jobs":4,"priority":"BelowNormal","incremental":true}),
            );
            self.activity
                .compiling
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(cook) = &mut self.cook
            && let Some(status) = cook.child.try_wait()?
        {
            let cook = self.cook.take().context("completed cook")?;
            let relative = cook.manifest.strip_prefix(&self.root)?;
            let error = if status.success() {
                None
            } else {
                fs::read_to_string(&cook.log).ok()
            };
            let mut owners = self
                .claims
                .owners(relative)
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            if let Some(error) = &error {
                let error = error.replace('\\', "/").replace("//", "/");
                for (owner, paths) in &self.claims.0 {
                    if paths
                        .iter()
                        .any(|path| error.contains(&path.to_string_lossy().replace('\\', "/")))
                    {
                        owners.insert(owner.clone());
                    }
                }
            }
            let value = json!({"success":status.success(),"seconds":cook.started.elapsed().as_secs_f64(),"owners":owners,"log":cook.log,"error":error});
            self.shader_results
                .insert(relative.to_owned(), value.clone());
            self.event("shader_cook", value);
            let changed = fs::read_to_string(cook.output.join(".ara-shader-reload"))
                .unwrap_or_default()
                != cook.previous;
            if status.success() && changed {
                for session in self.sessions.values_mut() {
                    session.control(
                        json!({"reload_shaders":true,"measure":false}),
                        Duration::from_secs(30),
                    )?;
                }
            }
        }
        if self.cook.is_none()
            && admission.is_ok()
            && let Some(manifest) = self.cooks.pop_first()
        {
            if let Some(executable) = self.binaries.get("ara-shader-cook") {
                let output = super::watch::output(&self.root, &self.base, &manifest)?;
                fs::create_dir_all(&output)?;
                let log = self
                    .base
                    .join("cooks")
                    .join(format!("{}.log", session::id()));
                fs::create_dir_all(log.parent().context("cook log parent")?)?;
                let mut command = Command::new(executable);
                command
                    .current_dir(&self.root)
                    .arg(&manifest)
                    .arg(&output)
                    .stdout(File::options().create(true).append(true).open(&log)?)
                    .stderr(File::options().append(true).open(&log)?);
                super::build::background(&mut command);
                self.cook = Some(Cook {
                    child: crate::process::Tree::spawn(&mut command)?,
                    manifest,
                    log,
                    started: Instant::now(),
                    previous: fs::read_to_string(output.join(".ara-shader-reload"))
                        .unwrap_or_default(),
                    output,
                });
                self.activity
                    .compiling
                    .store(true, std::sync::atomic::Ordering::Release);
            } else {
                self.cooks.insert(manifest);
            }
        }
        Ok(())
    }
    fn adopt(&mut self, binaries: crate::binaries::Binaries) -> Result<()> {
        let executable = super::session::Session::executable(&binaries)?;
        // Never retire a last-good player until its replacement has answered a control.
        let targets = self
            .sessions
            .values()
            .filter_map(Session::reload_target)
            .collect::<Vec<_>>();
        if !self.strategy.publish(&binaries, &targets)? {
            self.binaries = binaries;
            return Ok(());
        }
        for session in self.sessions.values() {
            session.restart(executable.to_owned())?;
        }
        self.binaries = binaries;
        session::write(&self.base.join("binaries.json"), &self.binaries)?;
        if self.sessions.is_empty() {
            let saved: BTreeMap<String, super::scenes::Selection> =
                fs::read(self.base.join("selections.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
            if saved.is_empty() {
                let selection =
                    super::scenes::select(&self.root, &self.base, &json!({}), &self.binaries)?;
                let session = Session::launch(
                    &self.root,
                    &self.base,
                    super::session::Session::executable(&self.binaries)?,
                    "default",
                    selection,
                    std::sync::Arc::clone(&self.activity),
                )?;
                self.sessions.insert("default".into(), session);
            } else {
                for (name, selection) in saved.into_iter().take(self.max_sessions as usize) {
                    let session = Session::launch(
                        &self.root,
                        &self.base,
                        super::session::Session::executable(&self.binaries)?,
                        &name,
                        selection,
                        std::sync::Arc::clone(&self.activity),
                    )?;
                    self.sessions.insert(name, session);
                }
            }
        }
        self.persist()?;
        Ok(())
    }
    fn dispatch(&mut self, request: Request) -> Result<Value> {
        let args = request.arguments;
        let name = args["session"].as_str().unwrap_or("default");
        match request.tool.as_str() {
            "lab_status" => self.status(),
            "lab_claim" => {
                let agent = args["agent"].as_str().context("agent required")?;
                let paths: Vec<String> = serde_json::from_value(args["paths"].clone())?;
                let warnings = self.claims.claim(agent, &paths)?;
                self.persist()?;
                Ok(json!({"agent":agent,"warnings":warnings}))
            }
            "lab_release" => {
                self.claims
                    .0
                    .remove(args["agent"].as_str().context("agent required")?);
                self.persist()?;
                Ok(json!({"released":true}))
            }
            "lab_rebuild" => {
                self.rebuild = true;
                Ok(json!({"queued":true,"strategy":self.strategy.name()}))
            }
            "lab_reload" => {
                let result = self
                    .sessions
                    .get_mut(name)
                    .context("open a track with lab_session first")?
                    .control(json!({"reload_shaders":true}), Duration::from_secs(60))?;
                Ok(
                    json!({"reload":result,"cooks_pending":self.cooks,"cook_errors":self.shader_results.values().filter(|result|result["success"]==false).collect::<Vec<_>>()}),
                )
            }
            "lab_session" => {
                session::name(name)?;
                ensure!(
                    self.sessions.contains_key(name)
                        || self.sessions.len() < self.max_sessions as usize,
                    "resident player limit reached; stop or reuse a track"
                );
                vram()?;
                let selection =
                    super::scenes::select(&self.root, &self.base, &args, &self.binaries)?;
                let player = Session::launch(
                    &self.root,
                    &self.base,
                    super::session::Session::executable(&self.binaries)?,
                    name,
                    selection,
                    std::sync::Arc::clone(&self.activity),
                )?;
                self.sessions.insert(name.into(), player);
                self.persist()?;
                self.sessions.get_mut(name).context("new session")?.status()
            }
            "lab_logs" => Ok(
                json!({"events":self.events.iter().rev().take(args["lines"].as_u64().unwrap_or(50).min(1000) as usize).collect::<Vec<_>>()}),
            ),
            "lab_stop" => {
                self.stopping = true;
                Ok(json!({"stopping":true}))
            }
            _ => bail!("unknown lab tool {}", request.tool),
        }
    }
}

pub(super) fn vram() -> Result<()> {
    #[cfg(windows)]
    {
        let mut command = Command::new("nvidia-smi");
        command.args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"]);
        super::build::background(&mut command);
        let output = command.output()?;
        ensure!(
            output.status.success(),
            "VRAM admission requires nvidia-smi"
        );
        let free = String::from_utf8(output.stdout)?
            .lines()
            .next()
            .context("VRAM sample")?
            .trim()
            .parse::<u64>()?;
        ensure!(
            free >= 2048,
            "only {free} MiB VRAM free; a warm track needs at least 2048 MiB headroom"
        );
    }
    Ok(())
}
pub(super) fn gpu_preflight() -> Result<()> {
    #[cfg(windows)]
    {
        let mut command = Command::new("nvidia-smi");
        command.args(["pmon", "-c", "1"]);
        super::build::background(&mut command);
        let output = command.output()?;
        ensure!(output.status.success(), "nvidia-smi pmon preflight failed");
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        // Presence and utilization are classified by the shared native guard, never this text table.
    }
    Ok(())
}
pub(super) fn run(root: &Path, target: &Path, max_sessions: u32) -> Result<bool> {
    super::run_with_strategy(root, target, max_sessions, Box::new(WarmRestart))
}

/// Hotpatch's CLI can enter the same daemon with its own build/publication strategy.
pub(crate) fn run_with_strategy(
    root: &Path,
    target: &Path,
    max_sessions: u32,
    strategy: Box<dyn ReloadStrategy>,
) -> Result<bool> {
    let base = super::directory(root);
    fs::create_dir_all(&base)?;
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(base.join("daemon.lock"))?;
    ensure!(
        lease.try_lock().is_ok(),
        "this checkout already has a lab daemon"
    );
    let target = if target.is_absolute() {
        target.to_owned()
    } else {
        root.join(target)
    };
    crate::build_paths::validate_output(root, &target)?;
    fs::create_dir_all(&target)?;
    let target = target.canonicalize()?;
    #[cfg(windows)]
    let target = PathBuf::from(
        target
            .to_string_lossy()
            .strip_prefix(r"\\?\")
            .unwrap_or(&target.to_string_lossy()),
    );
    session::write(
        &base.join("build-directory.json"),
        &json!({"directory":target}),
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let endpoint = Endpoint {
        address: listener.local_addr()?,
        token: session::id(),
        pid: std::process::id(),
    };
    session::write(&base.join("endpoint.json"), &endpoint)?;
    let claims = fs::read(base.join("claims.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut lab = Lab {
        root: root.into(),
        base: base.clone(),
        target,
        max_sessions,
        binaries: Default::default(),
        build: None,
        rebuild: true,
        last_build: Value::Null,
        cook: None,
        cooks: Default::default(),
        shader_results: Default::default(),
        watch: Watch::default(),
        claims,
        sessions: Default::default(),
        events: Default::default(),
        stopping: false,
        strategy,
        activity: std::sync::Arc::new(Activity::default()),
    };
    lab.watch.poll(root)?;
    for manifest in lab.watch.manifests() {
        let output = super::watch::output(root, &base, &manifest)?;
        let relative = output.strip_prefix(base.join("shaders"))?;
        let source = root.join(relative);
        if source.is_dir() && !output.exists() {
            super::watch::copy_tree(&source, &output)?;
        }
        if let (Some(source_parent), Some(output_parent)) = (source.parent(), output.parent()) {
            for name in ["atlas.rgba", "atlas.png"] {
                if source_parent.join(name).is_file() && !output_parent.join(name).exists() {
                    fs::create_dir_all(output_parent)?;
                    fs::copy(source_parent.join(name), output_parent.join(name))?;
                }
            }
        }
        if !source.is_dir() {
            lab.cooks.insert(manifest);
        }
    }
    let (send, receive) = std::sync::mpsc::channel::<(TcpStream, Result<Request>)>();
    while !lab.stopping {
        match listener.accept() {
            Ok((stream, _)) => {
                let send = send.clone();
                std::thread::spawn(move || {
                    let request = super::protocol::read(&stream);
                    let _ = send.send((stream, request));
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        while let Ok((stream, request)) = receive.try_recv() {
            if let Ok(request) = &request
                && request.token == endpoint.token
                && ["lab_measure", "lab_capture"].contains(&request.tool.as_str())
            {
                let name = request.arguments["session"].as_str().unwrap_or("default");
                if let Some(track) = lab.sessions.get(name) {
                    track.operation(request.tool.clone(), request.arguments.clone(), stream)?;
                } else {
                    super::protocol::reply(
                        stream,
                        Err(anyhow::anyhow!("open a track with lab_session first")),
                    )?;
                }
                continue;
            }
            let result = request.and_then(|request| {
                ensure!(request.token == endpoint.token, "foreign checkout token");
                lab.dispatch(request)
            });
            if let Err(error) = &result {
                lab.event("request_error", json!(format!("{error:#}")));
            }
            if let Err(error) = super::protocol::reply(stream, result) {
                lab.event("client_disconnected", json!(error.to_string()));
            }
        }
        if let Err(error) = lab.tick() {
            lab.event("iteration_error", json!(format!("{error:#}")));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(lab);
    fs::remove_file(base.join("endpoint.json"))?;
    drop(lease);
    Ok(true)
}
