//! The `ara_run` supervisor (R10 D5): agent C# scripts in a warm out-of-process worker
//! (`Ara.RunWorker`), compiled against the `Ara.Agent` facade only and checked against
//! an IL whitelist. The script's only write path is typed engine calls, which the
//! worker forwards here and the supervisor hands to a [`CommandSink`] (the Engine
//! Protocol plugs in there). Wall-clock and memory limits are enforced here: on a
//! violation the worker is killed and replaced by a warm spare.

mod job;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use ara_diag::Diagnostic;
use serde_json::{Value, json};

use crate::codes;
use job::Job;

/// Where a script's engine calls go: commands (writes, recorded as ops) and queries.
pub trait CommandSink {
    /// Runs `command` with JSON `params`; errors go back to the script as an exception.
    fn call(&mut self, command: &str, params: &Value) -> Result<Value, Diagnostic>;
}

/// Limits for one script run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    /// Wall-clock budget from submission to result (compile included).
    pub wall: Duration,
    /// Committed memory the worker may use, Roslyn and the runtime included.
    pub memory: u64,
}

impl Default for RunLimits {
    /// 10 s and 1 GiB.
    fn default() -> Self {
        RunLimits {
            wall: Duration::from_secs(10),
            memory: 1 << 30,
        }
    }
}

/// A finished script.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    /// The script's return value, as JSON (`null` when it returns nothing).
    pub result: Value,
    /// Why the changes were made (`ara.Why`), for the transaction review.
    pub why: Option<String>,
    /// Lines the script logged.
    pub log: Vec<String>,
    /// Every engine call, in order: `(command, params)`.
    pub calls: Vec<(String, Value)>,
}

/// One worker process: warm Roslyn, ready to run scripts one at a time.
pub struct RunWorker {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    job: Option<Job>,
    memory: u64,
    ready: bool,
    next_id: u64,
}

fn worker_error(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::RUN_WORKER, msg)
}

impl RunWorker {
    /// Starts a worker (`Ara.RunWorker.dll`, or its app host next to it) whose memory is
    /// capped at `memory` bytes. It warms up in the background; the first run waits for it.
    pub fn spawn(worker: &Path, memory: u64) -> Result<Self, Diagnostic> {
        let apphost = worker.with_extension(if cfg!(windows) { "exe" } else { "" });
        let mut cmd = if apphost.is_file() {
            Command::new(&apphost)
        } else {
            let mut c = Command::new("dotnet");
            c.arg("exec").arg(worker);
            c
        };
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env_remove("DOTNET_MODIFIABLE_ASSEMBLIES")
            // The managed heap throws OutOfMemoryException near the cap, on every platform.
            .env(
                "DOTNET_GCHeapHardLimit",
                format!("{:#x}", memory.saturating_sub(memory / 4)),
            );
        let mut child = cmd
            .spawn()
            .map_err(|e| worker_error(format!("start `{}`: {e}", worker.display())))?;
        let job = Job::new(&child, memory);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| worker_error("worker has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| worker_error("worker has no stdout"))?;
        let (tx, lines) = channel();
        std::thread::Builder::new()
            .name("ara-run-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| worker_error(format!("reader thread: {e}")))?;
        Ok(RunWorker {
            child,
            stdin,
            lines,
            job,
            memory,
            ready: false,
            next_id: 1,
        })
    }

    fn send(&mut self, v: &Value) -> Result<(), Diagnostic> {
        writeln!(self.stdin, "{v}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| worker_error(format!("write to worker: {e}")))
    }

    /// The worker's current committed memory, when the platform reports it.
    pub fn memory(&self) -> Option<u64> {
        self.job.as_ref().and_then(|j| j.committed(&self.child))
    }

    /// The most memory the worker has committed, when the platform reports it.
    pub fn peak_memory(&self) -> Option<u64> {
        self.job.as_ref().and_then(|j| j.peak(&self.child))
    }

    /// Kills the worker.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Runs one script. A limit violation kills the worker and returns `ARA0960`; the
    /// worker is unusable afterwards (the pool replaces it).
    pub fn run(
        &mut self,
        code: &str,
        limits: RunLimits,
        sink: &mut dyn CommandSink,
    ) -> Result<RunOutcome, Vec<Diagnostic>> {
        let deadline = Instant::now() + limits.wall;
        let limit = |what: String| vec![Diagnostic::error(codes::RUN_LIMIT, what)];
        if !self.ready {
            self.wait_ready(deadline).map_err(|d| vec![d])?;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "type": "run", "id": id, "code": code }))
            .map_err(|d| vec![d])?;
        let mut calls = Vec::new();
        loop {
            let now = Instant::now();
            if now >= deadline {
                self.kill();
                return Err(limit(format!(
                    "the script ran past its {} ms wall-clock limit and was killed",
                    limits.wall.as_millis()
                )));
            }
            if let Some(used) = self.memory()
                && used > limits.memory.min(self.memory)
            {
                self.kill();
                return Err(limit(format!(
                    "the worker used {} MiB, past its {} MiB limit, and was killed",
                    used >> 20,
                    limits.memory >> 20
                )));
            }
            let line = match self
                .lines
                .recv_timeout((deadline - now).min(Duration::from_millis(20)))
            {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    let peak = self.peak_memory().unwrap_or(0);
                    self.kill();
                    return Err(if peak >= self.memory.saturating_sub(self.memory / 8) {
                        limit(format!(
                            "the worker hit its {} MiB memory limit and died",
                            self.memory >> 20
                        ))
                    } else {
                        vec![worker_error("the worker exited during the script")]
                    });
                }
            };
            let msg: Value = serde_json::from_str(&line)
                .map_err(|e| vec![worker_error(format!("bad worker message: {e}"))])?;
            match msg["type"].as_str() {
                Some("call") => {
                    let command = msg["command"].as_str().unwrap_or_default().to_owned();
                    let params = msg["params"].clone();
                    let reply = match sink.call(&command, &params) {
                        Ok(output) => {
                            json!({ "type": "reply", "seq": msg["seq"], "ok": true, "output": output })
                        }
                        Err(d) => {
                            json!({ "type": "reply", "seq": msg["seq"], "ok": false, "code": d.code.to_string(), "message": d.message })
                        }
                    };
                    calls.push((command, params));
                    self.send(&reply).map_err(|d| vec![d])?;
                }
                Some("done") if msg["id"] == id => return self.finish(msg, calls),
                _ => {}
            }
        }
    }

    fn wait_ready(&mut self, deadline: Instant) -> Result<(), Diagnostic> {
        // Warm-up (Roslyn's first compile) gets at least 60 s, whatever the script's budget.
        let until = deadline.max(Instant::now() + Duration::from_secs(60));
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains("\"ready\"") => {
                    self.ready = true;
                    return Ok(());
                }
                Ok(_) => {}
                Err(_) => {
                    self.kill();
                    return Err(worker_error("the worker did not become ready"));
                }
            }
        }
    }

    fn finish(
        &mut self,
        msg: Value,
        calls: Vec<(String, Value)>,
    ) -> Result<RunOutcome, Vec<Diagnostic>> {
        if msg["ok"].as_bool() == Some(true) {
            return Ok(RunOutcome {
                result: msg["result"].clone(),
                why: msg["why"].as_str().map(str::to_owned),
                log: msg["log"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|l| l.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                calls,
            });
        }
        let diags: Vec<Diagnostic> = msg["diagnostics"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|d| serde_json::from_value(d.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        // An OutOfMemoryException inside the worker is a memory-limit violation: kill it.
        if diags
            .iter()
            .any(|d| d.message.contains("OutOfMemoryException"))
        {
            self.kill();
            return Err(vec![Diagnostic::error(
                codes::RUN_LIMIT,
                format!(
                    "the script hit the {} MiB memory limit; the worker was killed",
                    self.memory >> 20
                ),
            )]);
        }
        Err(if diags.is_empty() {
            vec![Diagnostic::error(codes::RUN_FAILED, "the script failed")]
        } else {
            diags
        })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for RunWorker {
    fn drop(&mut self) {
        let _ = self.send(&json!({ "type": "exit" }));
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            std::thread::sleep(Duration::from_millis(20));
            self.kill();
        }
    }
}

/// A worker with a warm spare, so a killed worker is replaced without paying for a
/// cold start on the next run.
pub struct RunPool {
    worker: PathBuf,
    memory: u64,
    active: Option<RunWorker>,
    spare: Option<RunWorker>,
}

impl RunPool {
    /// A pool of `Ara.RunWorker` processes capped at `memory` bytes each. Starts one
    /// worker and one spare (both warm up in the background).
    pub fn new(worker: impl Into<PathBuf>, memory: u64) -> Result<Self, Diagnostic> {
        let worker = worker.into();
        let active = Some(RunWorker::spawn(&worker, memory)?);
        let spare = Some(RunWorker::spawn(&worker, memory)?);
        Ok(RunPool {
            worker,
            memory,
            active,
            spare,
        })
    }

    /// Runs a script on the active worker, replacing it from the spare if it died or
    /// was killed (and starting a new spare).
    pub fn run(
        &mut self,
        code: &str,
        limits: RunLimits,
        sink: &mut dyn CommandSink,
    ) -> Result<RunOutcome, Vec<Diagnostic>> {
        if !self.active.as_mut().is_some_and(RunWorker::alive) {
            self.active = self.spare.take();
            self.spare = RunWorker::spawn(&self.worker, self.memory).ok();
        }
        let worker = self
            .active
            .as_mut()
            .ok_or_else(|| vec![worker_error("no worker available")])?;
        let out = worker.run(code, limits, sink);
        if !worker.alive() {
            self.active = None;
        }
        out
    }
}
