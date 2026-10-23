use crate::{
    Cancellation, Error, ImportContext, ImportInput, ImportOutput, ImporterRegistry, Progress,
    Result,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

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
    Failed(String),
}
#[derive(Serialize, Deserialize)]
enum Request {
    Import(ImportInput),
    Discover {
        importer: String,
        version: String,
        #[serde(with = "serde_bytes")]
        source: Vec<u8>,
    },
}
fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let bytes = rmp_serde::to_vec_named(value).map_err(|e| Error::Worker(e.to_string()))?;
    if bytes.len() as u64 > MAX_FRAME {
        return Err(Error::Worker("frame exceeds 512 MiB".into()));
    }
    writer.write_all(&(bytes.len() as u64).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
fn read_frame<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut len = [0; 8];
    reader.read_exact(&mut len)?;
    let len = u64::from_le_bytes(len);
    if len > MAX_FRAME {
        return Err(Error::Worker("frame exceeds 512 MiB".into()));
    }
    let mut bytes = vec![0; len as usize];
    reader.read_exact(&mut bytes)?;
    rmp_serde::from_slice(&bytes).map_err(|e| Error::Worker(e.to_string()))
}
/// Runs exactly one import in a disposable process. Plugins register in this worker.
/// Panics/native crashes terminate this process; no importer code runs in the host.
pub fn serve_worker(registry: &ImporterRegistry) -> Result<()> {
    background_priority();
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
    let input = match request {
        Request::Import(input) => input,
        Request::Discover {
            importer,
            version,
            source,
        } => {
            let result = registry.get(&importer).and_then(|i| {
                if i.version() != version {
                    return Err(Error::Invalid("importer version mismatch".into()));
                }
                i.dependencies(&source)
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
    let cancellation = Cancellation::default();
    let mut output = std::io::stdout();
    let mut transport_error = None;
    let result = registry.import(
        &input,
        &mut ImportContext {
            metrics: None,
            cancellation: &cancellation,
            progress: &mut |p| {
                if let Err(e) = write_frame(&mut output, &Message::Progress(p)) {
                    transport_error = Some(e);
                    cancellation.cancel();
                }
            },
        },
    );
    if let Some(e) = transport_error {
        return Err(e);
    }
    write_frame(
        &mut output,
        &match result {
            Ok(out) => Message::Complete(out),
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
    executable: PathBuf,
    arguments: Vec<std::ffi::OsString>,
    timeout: Duration,
}
impl ProcessWorker {
    /// Chooses the binary; production default deadline is ten minutes.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
            timeout: Duration::from_secs(600),
        }
    }
    /// Adds worker-specific configuration arguments (also useful for plugin hosts).
    pub fn with_arguments(
        mut self,
        args: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
    ) -> Self {
        self.arguments = args.into_iter().map(Into::into).collect();
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
    /// Spawns a below-normal-priority, hidden child and pumps both pipes off-thread.
    pub fn start(&self, input: ImportInput) -> Result<ImportJob> {
        self.start_request(Request::Import(input))
    }
    /// Discovers external files out of process; call from a snapshot/background job.
    pub fn discover(
        &self,
        importer: &str,
        version: &str,
        source: Vec<u8>,
        cancellation: &Cancellation,
    ) -> Result<Vec<String>> {
        let mut job = self.start_request(Request::Discover {
            importer: importer.into(),
            version: version.into(),
            source,
        })?;
        loop {
            cancellation.check()?;
            match job.poll()? {
                Some(JobEvent::Dependencies(deps)) => return Ok(deps),
                Some(JobEvent::Complete(_)) => {
                    return Err(Error::Worker("unexpected cook result".into()));
                }
                _ => thread::sleep(Duration::from_millis(2)),
            }
        }
    }
    fn start_request(&self, request: Request) -> Result<ImportJob> {
        let mut command = Command::new(&self.executable);
        command
            .args(&self.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(
                windows_sys::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS
                    | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
            );
        }
        let mut child = command.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Worker("missing stdin".into()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Worker("missing stdout".into()))?;
        let (tx, rx) = mpsc::channel();
        let writer_tx = tx.clone();
        thread::spawn(move || {
            background_priority();
            if let Err(e) = write_frame(&mut stdin, &request) {
                let _ = writer_tx.send(Err(e));
            }
        });
        thread::spawn(move || {
            background_priority();
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
        Ok(ImportJob {
            child: Some(child),
            events: rx,
            started: Instant::now(),
            timeout: self.timeout,
            finished: false,
        })
    }
}
/// A nonblocking process job. Dropping kills/reaps the child without joining on UI.
pub struct ImportJob {
    child: Option<Child>,
    events: Receiver<Result<Message>>,
    started: Instant,
    timeout: Duration,
    finished: bool,
}
impl ImportJob {
    /// Polls available progress/output; crashed workers become structured errors.
    pub fn poll(&mut self) -> Result<Option<JobEvent>> {
        if self.finished {
            return Ok(None);
        }
        if self.started.elapsed() > self.timeout {
            self.cancel();
            return Err(Error::Worker("deadline exceeded".into()));
        }
        match self.events.try_recv() {
            Ok(Ok(Message::Progress(p))) => Ok(Some(JobEvent::Progress(p))),
            Ok(Ok(Message::Complete(output))) => {
                self.finished = true;
                Ok(Some(JobEvent::Complete(output)))
            }
            Ok(Ok(Message::Dependencies(deps))) => {
                self.finished = true;
                Ok(Some(JobEvent::Dependencies(deps)))
            }
            Ok(Ok(Message::Failed(error))) => {
                self.finished = true;
                Err(Error::Worker(error))
            }
            Ok(Err(error)) => {
                self.finished = true;
                Err(error)
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                Err(Error::Worker("worker transport disconnected".into()))
            }
        }
    }
    /// Kills a crashed, hung or superseded native importer immediately.
    pub fn cancel(&mut self) {
        self.finished = true;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
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
