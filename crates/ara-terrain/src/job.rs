//! Cancellable below-normal-priority cooker and nonblocking completed-tile delivery.
use crate::{
    Tile, TileCoord,
    cache::{FileCache, TileSource},
    invalid,
};
use ara_diag::Diagnostic;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
};

/// Immutable cook request, prepared away from frame/tick work.
pub struct BakeRequest {
    /// Path to the ara-terrain-worker executable.
    pub worker: PathBuf,
    /// Saved canonical terrain manifest for this exact revision.
    pub manifest: PathBuf,
    /// Content-addressed binary directory.
    pub cache: PathBuf,
    /// Unique result path for this job; it is removed on completion/drop.
    pub output: PathBuf,
    /// Tile to evaluate.
    pub coord: TileCoord,
    /// Revision attached to the completed result for stale rejection.
    pub revision: u64,
}

/// One completed worker tile, ready for bounded frame-boundary publication.
pub struct BakedTile {
    /// Stable tile coordinate.
    pub coord: TileCoord,
    /// Source revision, used to reject obsolete results.
    pub revision: u64,
    /// Validated immutable payload.
    pub tile: Tile,
}

/// One process cook. `poll` never waits for compilation, cache I/O or decoding.
/// Drop cancels the process; active render data is owned separately by Residency.
pub struct BakeJob {
    child: Option<Child>,
    receiver: Option<Receiver<Result<Tile, Diagnostic>>>,
    request: BakeRequest,
    done: bool,
}
impl BakeJob {
    /// Starts a worker with bounded inputs; no visible console on Windows.
    pub fn spawn(request: BakeRequest) -> Result<Self, Diagnostic> {
        if request.output.exists() {
            return Err(invalid("terrain job output must be a fresh unique path"));
        }
        let mut command = Command::new(&request.worker);
        command
            .arg(&request.manifest)
            .arg(&request.cache)
            .arg(request.coord.x.to_string())
            .arg(request.coord.z.to_string())
            .arg(&request.output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000 | 0x00004000); // CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS
        }
        let child = command
            .spawn()
            .map_err(|e| invalid(format!("start terrain cooker: {e}")))?;
        Ok(Self {
            child: Some(child),
            receiver: None,
            request,
            done: false,
        })
    }
    /// Returns None while either process cooking or background payload decoding runs.
    pub fn poll(&mut self) -> Result<Option<BakedTile>, Diagnostic> {
        if self.done {
            return Ok(None);
        }
        if let Some(child) = &mut self.child {
            let Some(status) = child.try_wait().map_err(|e| invalid(e.to_string()))? else {
                return Ok(None);
            };
            self.child = None;
            if !status.success() {
                self.done = true;
                return Err(invalid(format!(
                    "terrain cooker exited {status}; last-good tile retained"
                )));
            }
            let output = self.request.output.to_owned();
            let cache = self.request.cache.to_owned();
            let (tx, rx) = mpsc::sync_channel(1);
            std::thread::Builder::new()
                .name("terrain-cache-read".into())
                .spawn(move || {
                    let result = (|| {
                        let text =
                            std::fs::read_to_string(&output).map_err(|e| invalid(e.to_string()))?;
                        let v = ara_json::parse(&text).map_err(|e| invalid(e.to_string()))?;
                        let hash = v
                            .get("hash")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| invalid("worker result lacks tile hash"))?;
                        FileCache::new(cache).load(hash)
                    })();
                    let _ = std::fs::remove_file(output);
                    let _ = tx.send(result);
                })
                .map_err(|e| invalid(e.to_string()))?;
            self.receiver = Some(rx);
            return Ok(None);
        }
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.done = true;
                    return result.map(|tile| {
                        Some(BakedTile {
                            coord: self.request.coord,
                            revision: self.request.revision,
                            tile,
                        })
                    });
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.done = true;
                    return Err(invalid("terrain payload reader disconnected"));
                }
            }
        }
        Ok(None)
    }
}
impl Drop for BakeJob {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.request.output);
    }
}
