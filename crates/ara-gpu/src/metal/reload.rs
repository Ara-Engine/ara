//! Development-only cohort compilation off the frame thread, with transactional publication.
use super::{MetalDevice, Retired, native, pipeline};
use crate::cooked::CookedPipeline;
use crate::{Pipeline, vulkan::ShaderStatus};
use anyhow::{Context, Result, ensure};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::MTLDevice;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct Entry {
    handle: Pipeline,
    manifest: PathBuf,
    active: CookedPipeline,
}
enum Request {
    Track(Entry),
    Forget(Pipeline),
    Reload,
    Stop,
}
struct Replacement {
    handle: Pipeline,
    native: pipeline::NativePipeline,
}
struct Compiled {
    key: String,
    values: Vec<Replacement>,
    sources: Vec<(Pipeline, CookedPipeline)>,
    load_ms: f64,
    pso_ms: f64,
}
enum Reply {
    Started,
    Idle,
    Forgotten(PathBuf),
    Ready {
        directory: PathBuf,
        key: String,
        values: Vec<Replacement>,
        milliseconds: f64,
        load_ms: f64,
        pso_ms: f64,
    },
    Failed {
        directory: PathBuf,
        error: String,
    },
}

pub(super) struct Worker {
    sender: Sender<Request>,
    receiver: Receiver<Reply>,
    thread: Option<JoinHandle<()>>,
    manifests: BTreeMap<PathBuf, ManifestIndex>,
    failures: BTreeMap<PathBuf, String>,
    pub status: ShaderStatus,
}
struct ManifestIndex {
    key: String,
    paths: BTreeMap<String, PathBuf>,
}

fn compatible(old: &CookedPipeline, new: &CookedPipeline) -> Result<()> {
    ensure!(
        old.version == new.version && old.binding_layout_hash == new.binding_layout_hash,
        "shader schema/layout changed; a Rust rebuild is required"
    );
    ensure!(
        serde_json::to_value(&old.state)? == serde_json::to_value(&new.state)?,
        "pipeline render state changed; a Rust rebuild is required"
    );
    ensure!(
        old.stages.len() == new.stages.len()
            && old
                .stages
                .iter()
                .zip(&new.stages)
                .all(|(a, b)| a.stage == b.stage
                    && a.source_entry == b.source_entry
                    && a.spirv_entry == b.spirv_entry),
        "pipeline stages changed; a Rust rebuild is required"
    );
    Ok(())
}

fn destroy(values: Vec<Replacement>) {
    drop(values);
}

fn compile(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &super::cache::Compiler,
    directory: &Path,
    entries: &[&Entry],
) -> Result<Compiled> {
    let metadata_started = Instant::now();
    let mut sources = Vec::new();
    for entry in entries {
        let source: CookedPipeline = serde_json::from_slice(&std::fs::read(&entry.manifest)?)?;
        compatible(&entry.active, &source)?;
        sources.push(source);
    }
    let key = sources.first().context("empty shader cohort")?.key.clone();
    ensure!(
        sources.iter().all(|s| s.key == key),
        "partially cooked shader cohort; retaining last good pipelines"
    );
    let mut load_ms = metadata_started.elapsed().as_secs_f64() * 1000.;
    let mut pso_ms = 0.;
    let mut values = Vec::new();
    for (entry, source) in entries.iter().zip(&sources) {
        if entry
            .active
            .stages
            .iter()
            .zip(&source.stages)
            .all(|(old, new)| {
                old.metal.as_ref().map(|m| &m.metallib_hash)
                    == new.metal.as_ref().map(|m| &m.metallib_hash)
            })
        {
            continue;
        }
        let prepared = native::pool(|| -> crate::Result<_> {
            let started = Instant::now();
            let libraries = source
                .stages
                .iter()
                .map(|stage| pipeline::load(device, source, stage, directory))
                .collect::<crate::Result<Vec<_>>>()?;
            load_ms += started.elapsed().as_secs_f64() * 1000.;
            let started = Instant::now();
            let result = pipeline::build_loaded(device, compiler, &libraries, &source.state);
            pso_ms += started.elapsed().as_secs_f64() * 1000.;
            result
        });
        match prepared {
            Ok(native) => values.push(Replacement {
                handle: entry.handle,
                native,
            }),
            Err(error) => {
                destroy(values);
                return Err(error.into());
            }
        }
    }
    Ok(Compiled {
        key,
        values,
        sources: entries
            .iter()
            .zip(sources)
            .map(|(entry, source)| (entry.handle, source))
            .collect(),
        load_ms,
        pso_ms,
    })
}

impl Worker {
    pub fn new(
        device: Retained<ProtocolObject<dyn MTLDevice>>,
        compiler: super::cache::Compiler,
    ) -> Result<Self> {
        let (sender, requests) = mpsc::channel();
        let (replies, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ara-shader-reload".into())
            .spawn(move || {
                let mut entries: Vec<Entry> = Vec::new();
                let mut seen: BTreeMap<PathBuf, String> = BTreeMap::new();
                loop {
                    let mut force = false;
                    match requests.recv_timeout(Duration::from_millis(200)) {
                        Ok(Request::Track(entry)) => {
                            if let Some(dir) = entry.manifest.parent() {
                                seen.entry(dir.to_owned()).or_insert_with(|| {
                                    std::fs::read_to_string(dir.join(".ara-shader-reload"))
                                        .unwrap_or_default()
                                });
                            }
                            entries.push(entry);
                        }
                        Ok(Request::Forget(handle)) => {
                            let directory = entries
                                .iter()
                                .find(|entry| entry.handle == handle)
                                .and_then(|entry| entry.manifest.parent())
                                .map(Path::to_owned);
                            entries.retain(|entry| entry.handle != handle);
                            if let Some(directory) = directory
                                && !entries.iter().any(|entry| {
                                    entry.manifest.parent() == Some(directory.as_path())
                                })
                            {
                                let _ = replies.send(Reply::Forgotten(directory));
                            }
                        }
                        Ok(Request::Reload) => force = true,
                        Ok(Request::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let mut groups: BTreeMap<PathBuf, Vec<&Entry>> = BTreeMap::new();
                    for entry in &entries {
                        if let Some(dir) = entry.manifest.parent() {
                            groups.entry(dir.to_owned()).or_default().push(entry);
                        }
                    }
                    let mut updates = Vec::new();
                    let mut started_batch = false;
                    for (directory, entries) in groups {
                        let token = std::fs::read_to_string(directory.join(".ara-shader-reload"))
                            .unwrap_or_default();
                        if !force && seen.get(&directory) == Some(&token) {
                            continue;
                        }
                        seen.insert(directory.clone(), token);
                        if !started_batch {
                            let _ = replies.send(Reply::Started);
                            started_batch = true;
                        }
                        let started = Instant::now();
                        let reply = match compile(&device, &compiler, &directory, &entries) {
                            Ok(compiled) => {
                                updates.extend(compiled.sources);
                                Reply::Ready {
                                    directory,
                                    key: compiled.key,
                                    values: compiled.values,
                                    milliseconds: started.elapsed().as_secs_f64() * 1000.,
                                    load_ms: compiled.load_ms,
                                    pso_ms: compiled.pso_ms,
                                }
                            }
                            Err(error) => Reply::Failed {
                                error: format!("{}: {error:#}", directory.display()),
                                directory,
                            },
                        };
                        if let Err(mpsc::SendError(Reply::Ready { values, .. })) =
                            replies.send(reply)
                        {
                            destroy(values);
                        }
                    }
                    if started_batch || force {
                        let _ = replies.send(Reply::Idle);
                    }
                    for (handle, source) in updates {
                        if let Some(entry) = entries.iter_mut().find(|entry| entry.handle == handle)
                        {
                            entry.active = source;
                        }
                    }
                }
            })?;
        Ok(Self {
            sender,
            receiver,
            thread: Some(thread),
            status: ShaderStatus::default(),
            manifests: BTreeMap::new(),
            failures: BTreeMap::new(),
        })
    }

    pub fn track(
        &mut self,
        handle: Pipeline,
        cooked: &CookedPipeline,
        directory: &Path,
    ) -> Result<()> {
        fn signature(cooked: &CookedPipeline) -> Result<String> {
            Ok(serde_json::to_string(&(
                &cooked.binding_layout_hash,
                &cooked.state,
                cooked
                    .stages
                    .iter()
                    .map(|stage| (&stage.source_entry, stage.stage, &stage.spirv_hash))
                    .collect::<Vec<_>>(),
            ))?)
        }
        if self
            .manifests
            .get(directory)
            .is_none_or(|index| index.key != cooked.key)
        {
            let mut manifests = BTreeMap::new();
            for file in std::fs::read_dir(directory)? {
                let path = file?.path();
                if path.extension().is_some_and(|ext| ext == "json")
                    && let Ok(bytes) = std::fs::read(&path)
                    && let Ok(source) = serde_json::from_slice::<CookedPipeline>(&bytes)
                {
                    manifests.insert(signature(&source)?, path);
                }
            }
            self.manifests.insert(
                directory.to_owned(),
                ManifestIndex {
                    key: cooked.key.clone(),
                    paths: manifests,
                },
            );
        }
        if let Some(path) = self
            .manifests
            .get(directory)
            .and_then(|index| index.paths.get(&signature(cooked).ok()?))
        {
            self.status
                .cohorts
                .entry(directory.to_owned())
                .or_insert_with(|| cooked.key.clone());
            self.sender.send(Request::Track(Entry {
                handle,
                manifest: path.clone(),
                active: cooked.clone(),
            }))?;
        }
        Ok(())
    }

    pub fn forget(&self, handle: Pipeline) {
        let _ = self.sender.send(Request::Forget(handle));
    }
    pub fn request(&self) {
        let _ = self.sender.send(Request::Reload);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.sender.send(Request::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        while let Ok(reply) = self.receiver.try_recv() {
            if let Reply::Ready { values, .. } = reply {
                destroy(values);
            }
        }
    }
}

impl MetalDevice {
    /// Request background compilation; the frame keeps using its last good pipelines.
    pub fn request_shader_reload(&mut self) {
        if let Some(worker) = &mut self.shader_worker {
            worker.status.pending = true;
            worker.request();
        }
    }
    /// Snapshot active cohorts and reload progress, outside the frame hot path.
    pub fn shader_status(&self) -> ShaderStatus {
        self.shader_worker
            .as_ref()
            .map(|w| w.status.clone())
            .unwrap_or_default()
    }
    /// Current publication counter; unchanged when compilation or validation fails.
    pub fn shader_epoch(&self) -> u64 {
        self.shader_worker.as_ref().map_or(0, |w| w.status.epoch)
    }

    pub(super) fn poll_shader_reload(&mut self) {
        let Some(worker) = &mut self.shader_worker else {
            return;
        };
        while let Ok(reply) = worker.receiver.try_recv() {
            match reply {
                Reply::Started => worker.status.pending = true,
                Reply::Idle => worker.status.pending = false,
                Reply::Forgotten(directory) => {
                    worker.status.cohorts.remove(&directory);
                    worker.failures.remove(&directory);
                    worker.status.last_error = worker.failures.values().next().cloned();
                }
                Reply::Failed { directory, error } => {
                    eprintln!("Shader reload retained last good cohort: {error}");
                    worker.failures.insert(directory, error);
                    worker.status.last_error = worker.failures.values().next().cloned();
                }
                Reply::Ready {
                    directory,
                    key,
                    values,
                    milliseconds,
                    load_ms,
                    pso_ms,
                } => {
                    for value in values {
                        if let Ok(slot) = self.pipelines.get_mut(value.handle) {
                            let previous = std::mem::replace(slot, value.native);
                            self.retired.push(Retired::Pipeline {
                                _native: previous,
                                last: self.submitted,
                            });
                        } else {
                            destroy(vec![value]);
                        }
                    }
                    worker.status.epoch += 1;
                    worker.failures.remove(&directory);
                    worker.status.cohorts.insert(directory, key);
                    worker.status.compile_ms = milliseconds;
                    worker.status.load_ms = Some(load_ms);
                    worker.status.pso_ms = Some(pso_ms);
                    worker.status.last_error = worker.failures.values().next().cloned();
                    eprintln!(
                        "Shader epoch {} published ({milliseconds:.1} ms background compile)",
                        worker.status.epoch
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reload_rejects_layout_state_and_stage_changes() {
        let source: CookedPipeline = serde_json::from_value(serde_json::json!({"version":1,"key":"a","binding_layout_hash":"abi","compiler":"test","stages":[],"state":{"color_formats":[],"vertex_stride":0,"depth_test":false,"depth_write":false,"blend":false}})).unwrap();
        let mut next = source.clone();
        next.key = "b".into();
        assert!(compatible(&source, &next).is_ok());
        next.state.blend = true;
        assert!(compatible(&source, &next).is_err());
        next = source.clone();
        next.binding_layout_hash = "new".into();
        assert!(compatible(&source, &next).is_err());
    }
}
