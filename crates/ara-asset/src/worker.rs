use crate::{
    Cancellation, Error, ImportContext, ImportInput, ImportOutput, ImporterRegistry, Progress,
    Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

mod discovery;
mod encoder;
mod limit;
mod process;
use limit::{Admission, Work};
use process::Running;

const MAX_FRAME: u64 = 512 * 1024 * 1024;
pub(crate) fn background_priority() {
    #[cfg(windows)]
    {
        // SAFETY: GetCurrentThread returns a valid pseudo handle for this thread;
        // setting its scheduling priority changes no memory or handle ownership.
        unsafe {
            windows_sys::Win32::System::Threading::SetThreadPriority(
                windows_sys::Win32::System::Threading::GetCurrentThread(),
                windows_sys::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL,
            );
        }
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: Linux applies nice values per thread; pid 0 is this thread.
        // Raising its niceness only lowers scheduling priority and owns no memory.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, 5);
        }
    }
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        // SAFETY: This API changes only the calling thread's QoS. 0x11 is the
        // Darwin QOS_CLASS_UTILITY constant; relative priority zero is permitted.
        unsafe {
            pthread_set_qos_class_self_np(0x11, 0);
        }
    }
}
#[derive(Serialize, Deserialize)]
enum Message {
    Progress(Progress),
    Complete(ImportOutput),
    Dependencies(Vec<String>),
    Versions(BTreeMap<String, String>),
    Metadata {
        versions: BTreeMap<String, String>,
        encoder: Option<String>,
    },
    DiscoveryBatch(Vec<std::result::Result<Vec<String>, String>>),
    Failed(String),
    Encoder(Option<String>),
    EncoderHint(Option<String>),
    EncoderService {
        identity: String,
        endpoint: crate::EncoderEndpoint,
    },
    EncoderFailed(String),
}
#[derive(Serialize, Deserialize)]
enum Request {
    Import(#[serde(with = "encoder::shared_input")] std::sync::Arc<ImportInput>),
    ImportEncoded {
        #[serde(with = "encoder::shared_input")]
        input: std::sync::Arc<ImportInput>,
        backend: String,
    },
    Encoder,
    EncoderHint,
    Discover {
        importer: String,
        version: String,
        #[serde(with = "serde_bytes")]
        source: Vec<u8>,
        #[serde(default)]
        settings: serde_json::Value,
    },
    Versions,
    Metadata,
    DiscoverBatch(Vec<discovery::Input>),
}
fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let encode = crate::trace::Scope::new("ipc.encode", "frame");
    let bytes = rmp_serde::to_vec_named(value).map_err(|e| Error::Worker(e.to_string()))?;
    drop(encode);
    let _write = crate::trace::Scope::new("ipc.write", "frame").bytes(bytes.len());
    if bytes.len() as u64 > MAX_FRAME {
        return Err(Error::Worker("frame exceeds 512 MiB".into()));
    }
    writer.write_all(&(bytes.len() as u64).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
fn read_frame<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let _read = crate::trace::Scope::new("ipc.read", "frame");
    let mut len = [0; 8];
    reader.read_exact(&mut len)?;
    let len = u64::from_le_bytes(len);
    if len > MAX_FRAME {
        return Err(Error::Worker("frame exceeds 512 MiB".into()));
    }
    let mut bytes = vec![0; len as usize];
    reader.read_exact(&mut bytes)?;
    drop(_read);
    let _decode = crate::trace::Scope::new("ipc.decode", "frame").bytes(bytes.len());
    rmp_serde::from_slice(&bytes).map_err(|e| Error::Worker(e.to_string()))
}
/// Runs exactly one import in a disposable process. Plugins register in this worker.
/// Panics/native crashes terminate this process; no importer code runs in the host.
pub fn serve_worker(registry: &ImporterRegistry) -> Result<()> {
    serve(registry, None, None)
}
/// Constructs an optional encoder in the disposable worker, never in the host.
pub type EncoderFactory = fn(Option<&Path>) -> Result<Box<dyn crate::BlockEncoder>>;
/// Reads a validated capability memo without constructing a GPU device/service.
pub type EncoderHintFactory = fn(Option<&Path>) -> Result<Option<String>>;
/// Worker frontend with optional GPU encoding; ordinary requests remain CPU-only.
pub fn serve_worker_with_encoder(
    registry: &ImporterRegistry,
    factory: EncoderFactory,
) -> Result<()> {
    serve(registry, Some(factory), None)
}
/// Worker frontend with device-free capability hints and lazy miss negotiation.
pub fn serve_worker_with_encoder_hint(
    registry: &ImporterRegistry,
    factory: EncoderFactory,
    hint: EncoderHintFactory,
) -> Result<()> {
    serve(registry, Some(factory), Some(hint))
}
fn encoder_hint(hint: Option<EncoderHintFactory>) -> Option<String> {
    let root = std::env::var_os("ARA_IMPORT_MEMO_DIR").map(PathBuf::from);
    let _trace = crate::trace::Scope::new("texture.identity_lookup", "encoder");
    hint.and_then(|lookup| match lookup(root.as_deref()) {
        Ok(identity) => identity,
        Err(error) => {
            crate::trace::event(
                "texture.identity_miss",
                "encoder",
                serde_json::json!({"reason":error.to_string()}),
            );
            None
        }
    })
}
fn serve(
    registry: &ImporterRegistry,
    factory: Option<EncoderFactory>,
    hint: Option<EncoderHintFactory>,
) -> Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
        };
        // SAFETY: This worker-only process setting suppresses OS crash dialogs;
        // the host observes pipe closure and retains its last-good asset instead.
        unsafe {
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        }
    }
    let request: Request = read_frame(&mut std::io::stdin().lock())?;
    let (input, backend) = match request {
        Request::Import(input) => (input, None),
        Request::ImportEncoded { input, backend } => (input, Some(backend)),
        Request::EncoderHint => {
            let identity = encoder_hint(hint);
            return write_frame(
                &mut std::io::stdout().lock(),
                &Message::EncoderHint(identity),
            );
        }
        Request::Encoder => {
            let root = std::env::var_os("ARA_IMPORT_MEMO_DIR").map(PathBuf::from);
            let _prepare = crate::trace::Scope::new("texture.backend_prepare", "encoder");
            let encoder = factory.and_then(|make| {
                match make(root.as_deref()).and_then(|encoder| {
                    encoder.prepare()?;
                    Ok(encoder)
                }) {
                    Ok(encoder) => Some(encoder),
                    Err(error) => {
                        crate::trace::event(
                            "texture.backend_unavailable",
                            "encoder",
                            serde_json::json!({"reason":error.to_string()}),
                        );
                        None
                    }
                }
            });
            let identity = encoder.as_ref().map(|encoder| encoder.backend().to_owned());
            let endpoint = encoder
                .as_ref()
                .and_then(|encoder| encoder.service_endpoint());
            drop(encoder);
            drop(_prepare);
            return write_frame(
                &mut std::io::stdout().lock(),
                &match (identity, endpoint) {
                    (Some(identity), Some(endpoint)) => {
                        Message::EncoderService { identity, endpoint }
                    }
                    (identity, _) => Message::Encoder(identity),
                },
            );
        }
        Request::Versions => {
            return write_frame(
                &mut std::io::stdout().lock(),
                &Message::Versions(registry.versions()),
            );
        }
        Request::Metadata => {
            return write_frame(
                &mut std::io::stdout().lock(),
                &Message::Metadata {
                    versions: registry.versions(),
                    encoder: encoder_hint(hint),
                },
            );
        }
        Request::DiscoverBatch(requests) => {
            let results: Vec<_> = requests
                .iter()
                .map(|input| discovery::execute(registry, input).map_err(|error| error.to_string()))
                .collect();
            return write_frame(
                &mut std::io::stdout().lock(),
                &Message::DiscoveryBatch(results),
            );
        }
        Request::Discover {
            importer,
            version,
            source,
            settings,
        } => {
            let result = registry.get(&importer).and_then(|i| {
                if i.version() != version {
                    return Err(Error::Invalid("importer version mismatch".into()));
                }
                i.dependencies_with_settings(&source, &settings)
            });
            return write_frame(
                &mut std::io::stdout().lock(),
                &match result {
                    Ok(deps) => Message::Dependencies(deps),
                    Err(e) => Message::Failed(e.to_string()),
                },
            );
        }
    };
    background_priority();
    let cancellation = Cancellation::default();
    let memo = std::env::var_os("ARA_IMPORT_MEMO_DIR")
        .map(|root| crate::FileCache::intermediates(Path::new(&root)))
        .transpose()?;
    let metrics = crate::ImportMetrics::default();
    let traced = crate::trace::enabled();
    let mut output = std::io::stdout();
    let mut transport_error = None;
    let import_trace = crate::trace::Scope::new("worker.import", &input.source_path);
    let result = (|| {
        let encoder = backend
            .as_deref()
            .map(|requested| {
                let make = factory
                    .ok_or_else(|| Error::EncoderUnavailable("worker has no GPU encoder".into()))?;
                let root = std::env::var_os("ARA_IMPORT_MEMO_DIR").map(PathBuf::from);
                let encoder = make(root.as_deref())
                    .map_err(|error| Error::EncoderUnavailable(error.to_string()))?;
                if encoder.backend() != requested {
                    return Err(Error::EncoderUnavailable(
                        "cooked codec revision changed; retry with a fresh backend identity".into(),
                    ));
                }
                Ok(encoder)
            })
            .transpose()?;
        let result = registry.import_with_services(
            &input,
            &mut ImportContext {
                metrics: traced.then_some(&metrics),
                cancellation: &cancellation,
                progress: &mut |p| {
                    if let Err(e) = write_frame(&mut output, &Message::Progress(p)) {
                        transport_error = Some(e);
                        cancellation.cancel();
                    }
                },
            },
            crate::ImportServices {
                memo: memo.as_ref().map(|cache| cache as &dyn crate::CacheBackend),
                encoder: encoder.as_deref(),
            },
        );
        if traced && let Some(encoder) = &encoder {
            crate::trace::event(
                "texture.gpu",
                &input.source_path,
                serde_json::json!(encoder.metrics()),
            );
        }
        // Finish this worker's encoder/connection before Complete. Every encode
        // has resolved and read back; a leased device may outlive this process,
        // but none of this job's GPU work survives publication.
        drop(encoder);
        result
    })();
    drop(import_trace);
    if traced {
        crate::trace::event(
            "worker.stages",
            &input.source_path,
            serde_json::json!({
                "stages":metrics.snapshot(),"threads":input.thread_budget,"preset":input.preset,"encoder":backend,
                "peakRssBytes":crate::trace::peak_rss_bytes(),
                "sourceBytes":input.source.len(),"dependencyBytes":input.dependencies.values().map(Vec::len).sum::<usize>(),
                "outputBytes":result.as_ref().map_or(0,|o| o.artifacts.iter().map(|a| a.bytes.len()).sum::<usize>())
            }),
        );
    }
    if let Some(e) = transport_error {
        return Err(e);
    }
    write_frame(
        &mut output,
        &match result {
            Ok(out) => Message::Complete(out),
            Err(Error::EncoderUnavailable(error)) => Message::EncoderFailed(error),
            Err(e) => Message::Failed(e.to_string()),
        },
    )
}
/// Progress or terminal state emitted by a process job.
#[derive(Debug)]
pub enum JobEvent {
    /// Worker is still running.
    Progress(Progress),
    /// Entire output has passed validation; not yet installed in live slots.
    Complete(ImportOutput),
    /// Dependency discovery completed in the same crash-isolated worker host.
    Dependencies(Vec<String>),
}
/// Explicit worker executable and deadline, shared by CLI and editor.
#[derive(Debug, Clone)]
pub struct ProcessWorker {
    discovery: std::sync::Arc<discovery::State>,
    encoder: std::sync::Arc<encoder::State>,
    texture_encoder_enabled: bool,
    memo: Option<PathBuf>,
    executable: PathBuf,
    arguments: Vec<std::ffi::OsString>,
    timeout: Duration,
}
impl ProcessWorker {
    /// Chooses the binary; production default deadline is ten minutes.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            discovery: std::sync::Arc::default(),
            encoder: std::sync::Arc::default(),
            texture_encoder_enabled: true,
            memo: None,
            executable: executable.into(),
            arguments: Vec::new(),
            timeout: Duration::from_secs(600),
        }
    }
    /// Selects optional texture encoding for this handle. Disable for CPU-only
    /// contract tests or controlled ISPC measurements; ordinary imports default on.
    pub fn with_texture_encoder(mut self, enabled: bool) -> Self {
        self.texture_encoder_enabled = enabled;
        self
    }
    /// Adds worker-specific configuration arguments (also useful for plugin hosts).
    pub fn with_arguments(
        mut self,
        args: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
    ) -> Self {
        self.arguments = args.into_iter().map(Into::into).collect();
        self.discovery = std::sync::Arc::default();
        self.encoder = std::sync::Arc::default();
        self
    }
    /// Sets the native-code job deadline (cancellation can stop it earlier).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    /// Worker executable used to install matching importer plugins.
    pub fn executable(&self) -> &Path {
        &self.executable
    }
    /// Queues a hidden, below-normal-priority child, pumping its pipes off-thread.
    /// Admission is process-wide: `ARA_IMPORT_WORKERS` defaults to four, capped by
    /// available CPU parallelism. Polling starts queued work as slots become free.
    pub fn start(&self, input: ImportInput) -> Result<ImportJob> {
        self.start_request(Request::Import(std::sync::Arc::new(input)))
    }
    /// Discovers external files out of process; call from a snapshot/background job.
    pub fn discover(
        &self,
        importer: &str,
        version: &str,
        source: Vec<u8>,
        cancellation: &Cancellation,
    ) -> Result<Vec<String>> {
        self.discover_with_settings(
            importer,
            version,
            source,
            serde_json::Value::Null,
            cancellation,
        )
    }
    /// Discovers source and settings-selected companion files out of process.
    /// Use this for source snapshots so companion changes invalidate the cook.
    pub fn discover_with_settings(
        &self,
        importer: &str,
        version: &str,
        source: Vec<u8>,
        settings: serde_json::Value,
        cancellation: &Cancellation,
    ) -> Result<Vec<String>> {
        self.dependencies(
            discovery::Input {
                importer: importer.into(),
                version: version.into(),
                source,
                settings,
            },
            cancellation,
        )
    }
    /// The installed version of every importer in this worker executable (`id` →
    /// version). Blocks for one short worker launch; call off the frame thread.
    pub fn versions(&self) -> Result<BTreeMap<String, String>> {
        let mut job = self.start_request(Request::Versions)?;
        job.timeout = self.timeout.min(Duration::from_secs(30));
        loop {
            match job.message()? {
                Some(Message::Versions(versions)) => return Ok(versions),
                Some(Message::Failed(error)) => return Err(Error::Worker(error)),
                Some(_) => return Err(Error::Worker("unexpected worker reply".into())),
                None => thread::sleep(Duration::from_millis(2)),
            }
        }
    }
    /// Combines versions with a device-free encoder hint for a project open.
    /// Shares the hint with clones; old workers fall back to `versions` unchanged.
    /// Like `versions`, call from a background/CLI thread, never an editor frame.
    pub fn versions_with_encoder_hint(&self) -> Result<BTreeMap<String, String>> {
        if !self.texture_encoder_enabled {
            return self.versions();
        }
        let combined = (|| {
            let mut job = self.start_request(Request::Metadata)?;
            job.timeout = self.timeout.min(Duration::from_secs(30));
            loop {
                match job.message()? {
                    Some(Message::Metadata { versions, encoder }) => {
                        self.accept_encoder_hint(encoder);
                        return Ok(versions);
                    }
                    Some(_) => return Err(Error::Worker("unexpected worker metadata".into())),
                    None => thread::sleep(Duration::from_millis(2)),
                }
            }
        })();
        combined.or_else(|_| self.versions())
    }
    fn start_request(&self, request: Request) -> Result<ImportJob> {
        let mut command = Command::new(&self.executable);
        // Never inherit a different project's memo from the parent's environment.
        command.env_remove("ARA_IMPORT_MEMO_DIR");
        if matches!(
            &request,
            Request::Import(_)
                | Request::ImportEncoded { .. }
                | Request::Encoder
                | Request::EncoderHint
                | Request::Metadata
        ) && let Some(root) = &self.memo
        {
            command.env("ARA_IMPORT_MEMO_DIR", root);
        }
        command
            .args(&self.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(
                (if matches!(&request, Request::Import(_) | Request::ImportEncoded { .. }) {
                    windows_sys::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS
                } else {
                    windows_sys::Win32::System::Threading::NORMAL_PRIORITY_CLASS
                }) | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
            );
        }
        let (tx, rx) = mpsc::channel();
        let mut job = ImportJob {
            _encoder: std::sync::Arc::clone(&self.encoder),
            child: None,
            pending: Some(Pending {
                admission: Admission::new(match &request {
                    Request::Import(_) => Work::Cpu,
                    Request::ImportEncoded { .. } => Work::Encoded,
                    _ => Work::Metadata,
                }),
                _queue: crate::trace::Scope::new(
                    "worker.queue",
                    match &request {
                        Request::Import(input) => &input.source_path,
                        Request::ImportEncoded { input, .. } => &input.source_path,
                        Request::Encoder => "encoder",
                        Request::EncoderHint => "encoder-hint",
                        Request::Discover { importer, .. } => importer,
                        Request::Versions => "versions",
                        Request::Metadata => "metadata",
                        Request::DiscoverBatch(_) => "discovery-batch",
                    },
                ),
                command,
                request,
                events: tx,
            }),
            events: rx,
            started: Instant::now(),
            timeout: self.timeout,
            finished: false,
        };
        job.try_launch()?;
        Ok(job)
    }
}

impl ProcessWorker {
    /// Shares disposable dependency/texture metadata through an explicit cache.
    /// Cache availability never changes action keys or dependency results.
    pub fn with_memo(mut self, root: Option<&Path>) -> Self {
        let root = root.map(Path::to_path_buf);
        if self.memo != root {
            self.discovery = std::sync::Arc::default();
        }
        self.memo = root;
        self
    }
}

struct Pending {
    admission: Admission,
    _queue: crate::trace::Scope,
    command: Command,
    request: Request,
    events: mpsc::Sender<Result<Message>>,
}

/// A nonblocking queued/process job. Dropping cancels queued work or kills/reaps
/// its child without joining on UI. Worker deadlines exclude time in the queue.
pub struct ImportJob {
    // A queued/running job keeps a negotiated GPU service alive even when the
    // caller drops its ProcessWorker handle before consuming the result.
    _encoder: std::sync::Arc<encoder::State>,
    child: Option<Running>,
    pending: Option<Pending>,
    events: Receiver<Result<Message>>,
    started: Instant,
    timeout: Duration,
    finished: bool,
}
impl ImportJob {
    fn try_launch(&mut self) -> Result<()> {
        let Some(pending) = &self.pending else {
            return Ok(());
        };
        let Some(permit) = pending.admission.try_acquire() else {
            return Ok(());
        };
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        self.started = Instant::now();
        // A worker that cannot start (not built, wrong ARA_IMPORT_WORKER) names itself.
        let mut child = pending.command.spawn().map_err(|e| {
            Error::Worker(format!(
                "cannot start `{}`: {e}",
                Path::new(pending.command.get_program()).display()
            ))
        })?;
        if crate::trace::enabled() {
            crate::trace::event(
                "worker.spawn",
                "child",
                serde_json::json!({"child":child.id()}),
            );
        }
        let mut stdin = child
            .stdin
            .take()
            .expect("worker command configures piped stdin");
        let mut stdout = child
            .stdout
            .take()
            .expect("worker command configures piped stdout");
        self.child = Some(Running::new(child, permit));
        let tx = pending.events;
        let request = pending.request;
        let writer_tx = tx.clone();
        thread::spawn(move || {
            if let Err(e) = write_frame(&mut stdin, &request) {
                let _ = writer_tx.send(Err(e));
            }
        });
        thread::spawn(move || {
            loop {
                match read_frame::<Message>(&mut stdout) {
                    Ok(message) => {
                        if let Message::Complete(output) = &message
                            && let Err(error) = output.validate()
                        {
                            let _ = tx.send(Err(error));
                            break;
                        }
                        let terminal = !matches!(message, Message::Progress(_));
                        if tx.send(Ok(message)).is_err() || terminal {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(Error::Worker(format!(
                            "worker exited or closed output: {e}"
                        ))));
                        break;
                    }
                }
            }
        });
        Ok(())
    }
    /// Adds a nonblocking worker activity sample to the given CPU frame. Duration
    /// includes process startup/transport, so it is kept separate from CPU self time.
    #[cfg(feature = "profiling")]
    pub fn record_profile(&self, profiler: &mut ara_profile::Profiler, frame: u64) -> bool {
        let worker = self.child.as_ref().map_or(0, |child| u64::from(child.id()));
        profiler.job_activity(
            frame,
            "asset.import",
            worker,
            if self.pending.is_some() {
                0
            } else {
                self.started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
            },
            self.finished,
        )
    }
    /// Polls available progress/output; crashed workers become structured errors.
    pub fn poll(&mut self) -> Result<Option<JobEvent>> {
        #[cfg(feature = "profiling")]
        ara_profile::scope!("jobs.asset.poll");
        match self.message()? {
            Some(Message::Progress(p)) => Ok(Some(JobEvent::Progress(p))),
            Some(Message::Complete(output)) => Ok(Some(JobEvent::Complete(output))),
            Some(Message::Dependencies(deps)) => Ok(Some(JobEvent::Dependencies(deps))),
            Some(Message::Failed(error)) => Err(Error::Worker(error)),
            Some(Message::EncoderFailed(error)) => Err(Error::EncoderUnavailable(error)),
            Some(
                Message::Encoder(_) | Message::EncoderHint(_) | Message::EncoderService { .. },
            ) => Err(Error::Worker("unexpected encoder identity".into())),
            Some(Message::Versions(_) | Message::Metadata { .. }) => {
                Err(Error::Worker("unexpected importer metadata".into()))
            }
            Some(Message::DiscoveryBatch(_)) => {
                Err(Error::Worker("unexpected dependency batch".into()))
            }
            None => Ok(None),
        }
    }
    fn message(&mut self) -> Result<Option<Message>> {
        if self.finished {
            return Ok(None);
        }
        if let Err(error) = self.try_launch() {
            self.cancel();
            return Err(error);
        }
        if self.pending.is_some() {
            return Ok(None);
        }
        if self.started.elapsed() > self.timeout {
            self.cancel();
            return Err(Error::Worker("deadline exceeded".into()));
        }
        match self.events.try_recv() {
            Ok(message) => {
                if !matches!(message, Ok(Message::Progress(_))) {
                    self.cancel();
                }
                message.map(Some)
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.cancel();
                Err(Error::Worker("worker transport disconnected".into()))
            }
        }
    }
    /// Requests termination/reaping of a native importer without joining the caller.
    pub fn cancel(&mut self) {
        self.finished = true;
        self.pending = None;
        self.child = None;
    }
    /// Synchronous CLI/background convenience; editor frame code uses `poll`.
    pub fn wait(
        mut self,
        cancellation: &Cancellation,
        mut progress: impl FnMut(Progress),
    ) -> Result<ImportOutput> {
        cancellation.check()?;
        if self.finished {
            return Err(Error::Worker(
                "job has already completed or been cancelled".into(),
            ));
        }
        loop {
            if cancellation.is_cancelled() {
                self.cancel();
                return Err(Error::Cancelled);
            }
            match self.poll()? {
                Some(JobEvent::Complete(output)) => return Ok(output),
                Some(JobEvent::Progress(p)) => progress(p),
                Some(JobEvent::Dependencies(_)) => {
                    return Err(Error::Worker("unexpected dependency result".into()));
                }
                None => thread::sleep(Duration::from_millis(2)),
            }
        }
    }
}
impl Drop for ImportJob {
    fn drop(&mut self) {
        self.cancel();
    }
}
