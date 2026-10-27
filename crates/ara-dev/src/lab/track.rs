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
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(super) struct Activity {
    pub gpu: RwLock<()>,
    pub compiling: AtomicBool,
    starts: Mutex<()>,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            gpu: RwLock::new(()),
            compiling: AtomicBool::new(false),
            starts: Mutex::new(()),
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
    Restart(PathBuf),
}
pub(super) struct Track {
    pub selection: Selection,
    send: mpsc::Sender<Work>,
    status: mpsc::Receiver<Value>,
    latest: Value,
    cancel: Arc<AtomicBool>,
    pub selection_changed: bool,
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
        let (send, jobs) = mpsc::channel();
        let (publish, status) = mpsc::channel();
        let root = root.to_owned();
        let base = base.to_owned();
        let executable = executable.to_owned();
        let name = name.to_owned();
        let chosen = selection.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&cancel);
        let worker_name = format!("ara-lab-{name}");
        let run = move || {
            let initial = (|| -> Result<Session> {
                let _start = activity.starts.lock().expect("player admission");
                let _activity = activity.gpu.read().expect("lab activity");
                let _visual = crate::gpu::visual(600)?;
                super::daemon::vram()?;
                Session::launch(
                    &root,
                    &base,
                    &executable,
                    &name,
                    chosen,
                    Arc::clone(&stopped),
                )
            })();
            let mut player = match initial {
                Ok(player) => player,
                Err(error) => {
                    let message = format!("{error:#}");
                    let _ = publish.send(json!({"running":false,"error":message}));
                    while !stopped.load(Ordering::Acquire) {
                        match jobs.recv_timeout(Duration::from_millis(100)) {
                            Ok(Work::Operation { reply, .. }) => {
                                let _ = super::protocol::reply(
                                    reply,
                                    Err(anyhow::anyhow!("track startup failed: {message}")),
                                );
                            }
                            Ok(_) => {}
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(_) => break,
                        }
                    }
                    return;
                }
            };
            let mut executable = executable;
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
                                break;
                            }
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                let _=publish.send(json!({"busy":true,"pid":player.child.id(),"directory":player.directory,"selection":player.selection}));
                match work {
                    Work::Restart(next) => {
                        let outcome = (|| -> Result<Session> {
                            let _start = activity.starts.lock().expect("player admission");
                            let _activity = activity.gpu.read().expect("lab activity");
                            let _visual = crate::gpu::visual(600)?;
                            super::daemon::vram()?;
                            Session::launch(
                                &root,
                                &base,
                                &next,
                                &name,
                                player.selection.clone(),
                                Arc::clone(&stopped),
                            )
                        })();
                        match outcome {
                            Ok(next_player) => {
                                player = next_player;
                                executable = next;
                            }
                            Err(error) => {
                                let _=publish.send(json!({"restart_error":format!("{error:#}"),"last_good_retained":true}));
                            }
                        }
                    }
                    Work::Control(request) => {
                        let result = (|| -> Result<Value> {
                            let _activity = activity.gpu.read().expect("lab activity");
                            let _visual = crate::gpu::visual(600)?;
                            player.control(request, Duration::from_secs(60))
                        })();
                        if let Err(error) = result {
                            let _ = publish.send(json!({"reload_error":format!("{error:#}")}));
                        }
                    }
                    Work::Operation { tool, args, reply } => {
                        let started = std::time::Instant::now();
                        let result = (|| -> Result<Value> {
                            let mut value = match tool.as_str() {
                                "lab_measure" => {
                                    let _exclusive = activity.gpu.write().expect("lab activity");
                                    ensure!(
                                        !activity.compiling.load(Ordering::Acquire),
                                        "wait for the lab's build/cook lane before timing"
                                    );
                                    super::daemon::gpu_preflight()?;
                                    let frames = args["frames"].as_u64().unwrap_or(120);
                                    ensure!((1..=100000).contains(&frames), "invalid frame count");
                                    let preset = args["preset"].as_str().unwrap_or("high");
                                    let policy: crate::gpu_guard::Policy = serde_json::from_value(
                                        args.get("guard").cloned().unwrap_or_else(|| json!({})),
                                    )?;
                                    let measured = crate::gpu::locked_with(
                                        600,
                                        !args["no_wait"].as_bool().unwrap_or(false),
                                        policy,
                                        || player.measure(frames as u32, preset),
                                    );
                                    match measured {
                                        Ok(mut value) => {
                                            value["timing_guard"] =
                                                json!(crate::gpu::last_reading());
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
                                                &json!({"error":format!("{error:#}"),"timing_guard":crate::gpu::last_reading()}),
                                            )?;
                                            return Err(error);
                                        }
                                    }
                                }
                                "lab_capture" => {
                                    let _activity = activity.gpu.read().expect("lab activity");
                                    let _visual = crate::gpu::visual(600)?;
                                    let output = base.join("captures").join(session::id());
                                    player.capture(&root, &output)?
                                }
                                _ => anyhow::bail!("unknown track operation"),
                            };
                            value["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
                            value["pid"] = json!(player.child.id());
                            value["selection"] = json!(player.selection);
                            value["executable"] = json!(executable);
                            if tool == "lab_measure" {
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
        std::thread::Builder::new().name(worker_name).spawn(run)?;
        Ok(Self {
            selection,
            send,
            status,
            latest: json!({"starting":true}),
            cancel,
            selection_changed: false,
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
        Ok(self.send.send(Work::Restart(executable))?)
    }
    pub fn operation(&self, tool: String, args: Value, reply: TcpStream) -> Result<()> {
        Ok(self.send.send(Work::Operation { tool, args, reply })?)
    }
    pub fn reload_target(&self) -> Option<super::build::ReloadTarget> {
        Some(super::build::ReloadTarget {
            pid: self.latest["pid"].as_u64()? as u32,
            control: PathBuf::from(self.latest["directory"].as_str()?).join("control"),
        })
    }
}
impl Drop for Track {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
