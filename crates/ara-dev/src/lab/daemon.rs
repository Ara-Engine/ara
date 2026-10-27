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

mod dispatch;
mod shaders;

struct Cook {
    child: crate::process::Tree,
    manifest: PathBuf,
    log: PathBuf,
    started: Instant,
    output: PathBuf,
    previous: String,
    variant: String,
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
    cooks: BTreeSet<(String, PathBuf)>,
    shader_results: BTreeMap<PathBuf, Value>,
    watch: Watch,
    claims: Claims,
    sessions: BTreeMap<String, Session>,
    events: VecDeque<Value>,
    stopping: bool,
    strategy: Box<dyn ReloadStrategy>,
    activity: std::sync::Arc<Activity>,
    variants: super::variants::Variants,
    pending_variants: BTreeMap<String, String>,
    promotions: BTreeSet<(String, PathBuf)>,
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
            json!({"format":"ara.lab/1","pid":std::process::id(),"root":self.root,"build_dir":self.target,"strategy":self.strategy.name(),"building":self.build.is_some(),"rebuild_queued":self.rebuild,"last_build":self.last_build,"shader_packages":self.watch.manifests().len(),"shader_queue":self.cooks,"shader_results":self.shader_results,"claims":self.claims,"sessions":sessions,"max_sessions":self.max_sessions,"variants":self.variants.status(),"pending_variants":self.pending_variants}),
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
        // Filesystem scanning is background CPU work too. Pause it while timing
        // the immutable player generation under the exclusive GPU lease.
        let changes = if let Ok(_observation) = self.activity.gpu.try_read() {
            self.watch.poll(&self.root)?
        } else {
            Vec::new()
        };
        for path in &changes {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ["rs", "toml", "lock"].contains(&ext) {
                self.rebuild = true;
                self.activity
                    .rust_latest
                    .store(false, std::sync::atomic::Ordering::Release);
            }
            for manifest in self.watch.manifests() {
                if path.starts_with(manifest.parent().context("shader source parent")?) {
                    self.cooks.insert(("baseline".into(), manifest.clone()));
                    let relative = manifest.strip_prefix(&self.root)?;
                    for name in self.variants.affected(relative) {
                        self.variants.queued(&name, relative);
                        self.cooks.insert((name, manifest.clone()));
                    }
                }
            }
        }
        if !changes.is_empty() {
            self.variants.save(&self.base)?;
            self.event("source_changes", json!(changes));
        }
        if let Some(build) = &mut self.build
            && let Some(status) = build.child.try_wait()?
        {
            let build = self.build.take().context("completed build")?;
            self.last_build = build.outcome(status.success(), &self.root, &self.claims)?;
            if self.rebuild {
                self.last_build["superseded"] = json!(true);
            }
            if status.success() && self.rebuild && !self.strategy.publish_superseded() {
                self.last_build["superseded"] = json!(true);
                self.event("newer_sources_queued", self.last_build.clone());
            } else if status.success() {
                match self
                    .strategy
                    .stage(&build.directory, &self.base)
                    .and_then(|binaries| self.adopt(binaries))
                {
                    Ok(()) => {
                        self.activity
                            .rust_latest
                            .store(!self.rebuild, std::sync::atomic::Ordering::Release);
                        self.event("build_published", self.last_build.clone())
                    }
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
        drop(admission);
        self.tick_shaders()
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
        session::write(
            &self.base.join("generation.json"),
            &json!({"id":session::id()}),
        )?;
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
        match super::generations::prune(&self.base) {
            Ok(removed) if removed > 0 => {
                self.event("player_cache_pruned", json!({"generations":removed}))
            }
            Err(error) => self.event("player_cache_cleanup_error", json!(error.to_string())),
            _ => {}
        }
        Ok(())
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
        shader_results: session::read(&base.join("shader-results.json"))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
        watch: Watch::default(),
        claims,
        sessions: Default::default(),
        events: Default::default(),
        stopping: false,
        strategy,
        activity: std::sync::Arc::new(Activity::default()),
        variants: super::variants::Variants::load(&base),
        pending_variants: Default::default(),
        promotions: Default::default(),
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
            lab.cooks.insert(("baseline".into(), manifest));
        }
    }
    for (name, relative) in lab.variants.outstanding() {
        lab.cooks.insert((name, root.join(relative)));
    }
    for (path, value) in &lab.shader_results {
        if value["variant"] == "baseline"
            && value["success"] == false
            && let Ok(relative) = path.strip_prefix("baseline")
        {
            lab.cooks.insert(("baseline".into(), root.join(relative)));
        }
    }
    // Start a cached last-good generation before the incremental build. A bad edit
    // or a tool upgrade must not turn a warm restart into an empty lab.
    if let Some(binaries) = session::read(&base.join("binaries.json"))
        .and_then(|v| serde_json::from_value::<crate::binaries::Binaries>(v).ok())
        && binaries
            .get("ara-player")
            .is_some_and(|path| path.is_file())
    {
        lab.adopt(binaries)?;
    }
    let mut dirty = crate::git::text(
        root,
        ["diff", "--name-only", "HEAD", "--", "crates", "shaders"],
    )?;
    dirty.push_str(&format!(
        "\n{}",
        crate::git::text(
            root,
            [
                "ls-files",
                "--others",
                "--exclude-standard",
                "--",
                "crates",
                "shaders"
            ]
        )?
    ));
    for path in dirty.lines().map(|p| root.join(p)) {
        if path
            .extension()
            .is_some_and(|e| e == "slang" || e == "slangh" || e == "h")
            || path.file_name().is_some_and(|n| n == "pipelines.json")
        {
            for manifest in lab.watch.manifests() {
                if path.starts_with(manifest.parent().context("shader parent")?) {
                    lab.cooks.insert(("baseline".into(), manifest.clone()));
                    for name in lab.variants.affected(manifest.strip_prefix(root)?) {
                        lab.variants.queued(&name, manifest.strip_prefix(root)?);
                        lab.cooks.insert((name, manifest.clone()));
                    }
                }
            }
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
                && request.tool == "lab_reference"
            {
                let root = lab.root.clone();
                let base = lab.base.clone();
                let args = request.arguments.clone();
                let activity = std::sync::Arc::clone(&lab.activity);
                std::thread::spawn(move || {
                    let result = super::references::render(&root, &base, &args, &activity);
                    let _ = super::protocol::reply(stream, result);
                });
                continue;
            }
            if let Ok(request) = &request
                && request.token == endpoint.token
                && ["lab_measure", "lab_capture", "lab_ab", "lab_set"]
                    .contains(&request.tool.as_str())
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
