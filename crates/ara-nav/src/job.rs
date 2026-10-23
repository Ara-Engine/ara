//! D20 worker process lifecycle: below-normal priority, cancellation, no tick waits.
use crate::{BakeArtifact, BakeInput, Baker, NavError, NavMesh};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    time::Duration,
};

const MAX_JOB_BYTES: u64 = 512 * 1024 * 1024;

/// Complete reproducible worker request; raw geometry can come from physics/assets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRequest {
    /// Current geometry, obstacles, links, tile set and profile.
    pub input: BakeInput,
    /// Last bake, for incremental reuse (not the currently published gameplay state).
    pub previous: Option<BakeArtifact>,
    /// Optional persistent content-addressed cache directory.
    pub cache: Option<PathBuf>,
}

/// Nonblocking worker observation. A result is consumed once.
pub enum JobPoll {
    /// Native bake or assembly is still running in the background.
    Pending,
    /// Validated artifact and ready-to-publish native mesh, or a structured error.
    Ready(Result<(BakeArtifact, NavMesh), NavError>),
    /// The result was already consumed.
    Finished,
}

/// Out-of-process background bake; dropping cancels without waiting on the tick.
pub struct BakeJob {
    result: Receiver<Result<(BakeArtifact, NavMesh), NavError>>,
    cancel: Sender<()>,
    finished: bool,
}
impl BakeJob {
    /// Queues serialization and process creation on a coordinator thread. The
    /// explicit worker path is normally `ara-nav-worker` beside the editor/daemon.
    pub fn spawn(worker: PathBuf, request: WorkerRequest) -> Result<Self, NavError> {
        let (tx, result) = mpsc::channel();
        let (cancel, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ara-nav-coordinator".to_owned())
            .spawn(move || {
                let response = run_worker(&worker, request, &rx);
                let _ = tx.send(response);
            })?;
        Ok(Self {
            result,
            cancel,
            finished: false,
        })
    }
    /// Reads readiness without waiting. Publication remains a caller-controlled
    /// tick-boundary action, respecting Play and agent-approval policy.
    pub fn poll(&mut self) -> JobPoll {
        if self.finished {
            return JobPoll::Finished;
        }
        match self.result.try_recv() {
            Ok(result) => {
                self.finished = true;
                JobPoll::Ready(result)
            }
            Err(TryRecvError::Empty) => JobPoll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                JobPoll::Ready(Err(NavError::new(
                    "worker",
                    "worker coordinator terminated",
                )))
            }
        }
    }
    /// Requests cancellation; the coordinator kills/reaps the worker off-thread.
    pub fn cancel(&self) {
        let _ = self.cancel.send(());
    }
}
impl Drop for BakeJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct JobDirectory(PathBuf);
impl JobDirectory {
    fn new() -> Result<Self, NavError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let path = std::env::temp_dir().join(format!(
                "ara-nav-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(NavError::new(
            "worker",
            "could not reserve a private job directory",
        ))
    }
}
impl Drop for JobDirectory {
    fn drop(&mut self) {
        // This directory was exclusively created above, never derived from user
        // input. The worker is reaped before this guard drops.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn run_worker(
    worker: &Path,
    request: WorkerRequest,
    cancel: &Receiver<()>,
) -> Result<(BakeArtifact, NavMesh), NavError> {
    let directory = JobDirectory::new()?;
    let input = directory.0.join("request.bin");
    let output = directory.0.join("result.bin");
    let bytes =
        postcard::to_allocvec(&request).map_err(|e| NavError::new("encode", e.to_string()))?;
    if bytes.len() as u64 > MAX_JOB_BYTES {
        return Err(NavError::new("capacity", "worker request exceeds 512 MiB"));
    }
    std::fs::write(&input, bytes)?;
    if cancel.try_recv().is_ok() {
        return Err(NavError::new("cancelled", "bake cancelled before launch"));
    }
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new(worker);
        c.creation_flags(0x00004000 | 0x08000000); // BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = Command::new("nice");
        c.args(["-n", "10"]).arg(worker);
        c
    };
    let mut child = command
        .arg(&input)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    loop {
        if cancel.try_recv().is_ok() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(NavError::new("cancelled", "bake worker cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(NavError::new(
                        "worker",
                        format!("bake worker exited with {status}; retain last good navmesh"),
                    ));
                }
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.into());
            }
        }
    }
    let response: Result<BakeArtifact, NavError> = decode(&output)?;
    let artifact = response?;
    let mesh = NavMesh::from_artifact(&artifact)?;
    Ok((artifact, mesh))
}
fn decode<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, NavError> {
    if std::fs::metadata(path)?.len() > MAX_JOB_BYTES {
        return Err(NavError::new("capacity", "worker file exceeds 512 MiB"));
    }
    postcard::from_bytes(&std::fs::read(path)?).map_err(|e| NavError::new("decode", e.to_string()))
}

/// Entry point of `ara-nav-worker`; intended for the engine's process launcher.
/// Positional arguments are the private request file and result file.
pub fn worker_main() -> Result<(), NavError> {
    let mut args = std::env::args_os().skip(1);
    let input = args
        .next()
        .ok_or_else(|| NavError::new("arguments", "missing request path"))?;
    let output = args
        .next()
        .ok_or_else(|| NavError::new("arguments", "missing result path"))?;
    let response: Result<BakeArtifact, NavError> = (|| {
        let request: WorkerRequest = decode(Path::new(&input))?;
        Baker::new(request.cache).bake(&request.input, request.previous.as_ref())
    })();
    let bytes =
        postcard::to_allocvec(&response).map_err(|e| NavError::new("encode", e.to_string()))?;
    crate::bake::atomic_write(Path::new(&output), &bytes)
}
