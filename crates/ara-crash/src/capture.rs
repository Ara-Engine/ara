#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use anyhow::Context;
use anyhow::{Result, bail};
use std::path::Path;

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
mod native {
    use super::*;
    use std::{
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32, Ordering},
        },
        time::{Duration, Instant},
    };

    static ATTACHED: AtomicBool = AtomicBool::new(false);
    type PanicHook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static;

    /// The exception a macOS panic snapshot records: SIGABRT as a Mach software
    /// exception (`EXC_SOFTWARE`, `EXC_SOFT_SIGNAL`), like Linux's simulated abort.
    #[cfg(target_os = "macos")]
    const ABORT: minidump_writer::crash_context::ExceptionInfo =
        minidump_writer::crash_context::ExceptionInfo {
            kind: crash_handler::ExceptionType::Software as u32,
            code: 0x10003,
            subcode: Some(libc::SIGABRT as u64),
        };

    /// Owns the native crash handler and the monitor child. Drop detaches and reaps.
    pub struct Capture {
        handler: Option<Arc<crash_handler::CrashHandler>>,
        child: Child,
        socket: PathBuf,
        previous_hook: Arc<PanicHook>,
    }
    impl Capture {
        /// Starts the current executable in monitor mode and installs local handlers.
        /// Call `monitor_from_args` first in that executable's entry point.
        pub fn start(directory: &Path, build_id: &str) -> Result<Self> {
            anyhow::ensure!(
                !ATTACHED.swap(true, Ordering::AcqRel),
                "ARA-EXP-CRASH-004: a crash handler is already installed"
            );
            let result = Self::attach(directory, build_id);
            if result.is_err() {
                ATTACHED.store(false, Ordering::Release);
            }
            result
        }
        fn attach(directory: &Path, build_id: &str) -> Result<Self> {
            std::fs::create_dir_all(directory)?;
            let directory = std::fs::canonicalize(directory)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let socket =
                std::env::temp_dir().join(format!("ara-{}-{stamp}.sock", std::process::id()));
            let socket_name = socket.to_str().context("non-UTF8 crash socket path")?;
            let mut command = Command::new(std::env::current_exe()?);
            command
                .arg("--ara-crash-monitor")
                .arg(&socket)
                .arg(&directory)
                .arg(build_id)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }
            let mut child = command
                .spawn()
                .context("ARA-EXP-CRASH-004: starting crash monitor")?;
            let connected = (|| {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if let Ok(client) =
                        minidumper::Client::with_name(minidumper::SocketName::path(socket_name))
                    {
                        break Ok(client);
                    }
                    if child.try_wait()?.is_some() || Instant::now() >= deadline {
                        bail!("ARA-EXP-CRASH-004: crash monitor did not become ready");
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })();
            let client = match connected {
                Ok(client) => Arc::new(client),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&socket);
                    return Err(error);
                }
            };
            let panic_client = Arc::downgrade(&client);
            let snapshot = Arc::new(AtomicU32::new(0));
            let in_snapshot = snapshot.clone();
            #[cfg(target_os = "macos")]
            let (pending, in_pending, snapshot_socket) = {
                let pending = Arc::new(crate::snapshot::Pending::default());
                (pending.clone(), pending, socket_name.to_owned())
            };
            // SAFETY: the callback owns the connected client for the handler's lifetime.
            // It performs only minidumper's signal-safe IPC; all allocation and I/O
            // for the dump happens in the monitor process.
            let event = unsafe {
                crash_handler::make_crash_event(move |context: &crash_handler::CrashContext| {
                    let snapshot =
                        in_snapshot.load(Ordering::Acquire) == crate::snapshot::thread(context);
                    #[cfg(target_os = "macos")]
                    let handled = if snapshot {
                        in_pending.request(context)
                    } else {
                        client.request_dump(context).is_ok()
                    };
                    #[cfg(not(target_os = "macos"))]
                    let handled = if snapshot {
                        crate::snapshot::request(&client, context)
                    } else {
                        client.request_dump(context).is_ok()
                    };
                    crash_handler::CrashEventResult::Handled(handled)
                })
            };
            let handler = match crash_handler::CrashHandler::attach(event) {
                Ok(handler) => handler,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&socket);
                    return Err(error.into());
                }
            };
            #[cfg(target_os = "linux")]
            handler.set_ptracer(Some(child.id()));
            let handler = Arc::new(handler);
            let panic_handler = Arc::downgrade(&handler);
            let previous_hook: Arc<PanicHook> = Arc::from(std::panic::take_hook());
            let previous = previous_hook.clone();
            let build = build_id.to_owned();
            let panic_lock = std::sync::Mutex::new(());
            std::panic::set_hook(Box::new(move |info| {
                // An old hook can survive a guard dropped during unwinding. Its
                // weak handler prevents duplicate reports and retained workers.
                let Some(handler) = panic_handler.upgrade() else {
                    previous(info);
                    return;
                };
                let report = crate::CrashReport::new(
                    &build,
                    crate::CrashKind::Panic,
                    info.to_string(),
                    std::backtrace::Backtrace::force_capture().to_string(),
                );
                let record = crate::write_report(&directory, &report);
                // macOS: the snapshot's own client names its record first (the ping
                // waits until the monitor has it), then dumps when the simulated
                // exception reaches the callback; this hook waits for that.
                #[cfg(target_os = "macos")]
                if let Ok(_guard) = panic_lock.try_lock()
                    && let Ok(record) = record
                    && let Some(name) = record.file_name().and_then(|n| n.to_str())
                    && let Ok(metadata) = crate::canonical(&serde_json::json!({"name":name}))
                    && let Ok(snapshot_client) = minidumper::Client::with_name(
                        minidumper::SocketName::path(&snapshot_socket),
                    )
                    && snapshot_client.send_message(1, metadata).is_ok()
                    && snapshot_client.ping().is_ok()
                {
                    pending.arm(snapshot_client);
                    snapshot.store(crate::snapshot::current_thread(), Ordering::Release);
                    let taken = handler.simulate_exception(Some(ABORT))
                        && pending.wait(Duration::from_secs(10));
                    snapshot.store(0, Ordering::Release);
                    if !taken && let Some(client) = panic_client.upgrade() {
                        // Unpair the record so a later crash keeps its own name.
                        let _ = client.send_message(3, []);
                    }
                }
                #[cfg(not(target_os = "macos"))]
                if let Ok(_guard) = panic_lock.try_lock()
                    && let Some(client) = panic_client.upgrade()
                    && let Ok(record) = record
                    && let Some(name) = record.file_name().and_then(|n| n.to_str())
                    && let Ok(metadata) = crate::canonical(&serde_json::json!({"name":name}))
                    && client.send_message(1, metadata).is_ok()
                {
                    snapshot.store(crate::snapshot::current_thread(), Ordering::Release);
                    #[cfg(windows)]
                    let _ = handler.simulate_exception(None);
                    #[cfg(target_os = "linux")]
                    let _ = handler.simulate_signal(crash_handler::Signal::Abort as u32);
                    snapshot.store(0, Ordering::Release);
                }
                previous(info);
            }));
            Ok(Self {
                handler: Some(handler),
                child,
                socket,
                previous_hook,
            })
        }
    }
    impl Drop for Capture {
        fn drop(&mut self) {
            self.handler.take();
            if !std::thread::panicking() {
                let previous = self.previous_hook.clone();
                std::panic::set_hook(Box::new(move |info| previous(info)));
            }
            // A completed dump is acknowledged before its client can terminate.
            // Normal shutdown explicitly kills/reaps the owned idle monitor.
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = std::fs::remove_file(&self.socket);
            ATTACHED.store(false, Ordering::Release);
        }
    }

    struct Handler {
        directory: PathBuf,
        build_id: String,
        panic: std::sync::Mutex<Option<String>>,
        /// The dump being written is a paired panic snapshot (macOS).
        #[cfg(target_os = "macos")]
        snapshot: AtomicBool,
    }
    impl minidumper::ServerHandler for Handler {
        fn create_minidump_file(&self) -> std::io::Result<(std::fs::File, PathBuf)> {
            // A macOS panic snapshot arrives as a dump request right after its record's
            // name, and takes the paired name.
            #[cfg(target_os = "macos")]
            let paired = self
                .panic
                .lock()
                .ok()
                .and_then(|mut pending| pending.take())
                .map(|name| crate::snapshot::path(&self.directory, &name))
                .transpose()
                .map_err(std::io::Error::other)?;
            #[cfg(target_os = "macos")]
            self.snapshot.store(paired.is_some(), Ordering::Release);
            #[cfg(not(target_os = "macos"))]
            let paired = None;
            let path = match paired {
                Some(path) => path,
                None => {
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(std::io::Error::other)?
                        .as_nanos();
                    self.directory
                        .join(format!("{}-{stamp}.dmp", std::process::id()))
                }
            };
            let file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .read(true)
                .open(&path)?;
            Ok((file, path))
        }
        fn on_minidump_created(
            &self,
            result: std::result::Result<minidumper::MinidumpBinary, minidumper::Error>,
        ) -> minidumper::LoopAction {
            if let Ok(dump) = result {
                let _ = dump.file.sync_all();
                let report = crate::CrashReport::new(
                    &self.build_id,
                    crate::CrashKind::Native,
                    "Native exception or fatal signal; symbolize the minidump for details".into(),
                    String::new(),
                );
                // A panic snapshot's dump sits beside the panic record it pairs with;
                // that record stays the report.
                let record = dump.path.with_extension("crash.json");
                if !record.exists()
                    && let Ok(json) = crate::canonical(&report)
                {
                    let _ = std::fs::write(record, json);
                }
            }
            // macOS: minidumper ends a crashed client's connection without a
            // disconnect callback, and each monitor serves one process, so it exits
            // after that process's fatal dump; a panic snapshot keeps it serving.
            #[cfg(target_os = "macos")]
            if !self.snapshot.load(Ordering::Acquire) {
                return minidumper::LoopAction::Exit;
            }
            // A caught Rust panic can continue running. The worker stays ready
            // until the owning process disconnects or drops its capture guard.
            minidumper::LoopAction::Continue
        }
        /// 1: a panic record's name; 2: its snapshot context (Windows, Linux);
        /// 3: the snapshot did not run (macOS), so a later dump keeps its own name.
        fn on_message(&self, kind: u32, buffer: Vec<u8>) {
            match kind {
                1 => {
                    if buffer.len() <= 60 * 1024
                        && let Ok(report) = serde_json::from_slice::<serde_json::Value>(&buffer)
                        && let Some(name) = report.get("name").and_then(|v| v.as_str())
                        && let Ok(mut pending) = self.panic.lock()
                    {
                        *pending = Some(name.to_owned());
                    }
                }
                #[cfg(not(target_os = "macos"))]
                2 => {
                    if let Some(name) = self
                        .panic
                        .lock()
                        .ok()
                        .and_then(|mut pending| pending.take())
                    {
                        let _ = crate::snapshot::write(&self.directory, &name, &buffer);
                    }
                }
                3 => {
                    if let Ok(mut pending) = self.panic.lock() {
                        pending.take();
                    }
                }
                _ => {}
            }
        }
        fn on_client_disconnected(&self, num_clients: usize) -> minidumper::LoopAction {
            if num_clients == 0 {
                minidumper::LoopAction::Exit
            } else {
                minidumper::LoopAction::Continue
            }
        }
    }

    pub fn monitor_from_args() -> Result<bool> {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--ara-crash-monitor")) {
            return Ok(false);
        }
        let socket = PathBuf::from(args.next().context("monitor socket missing")?);
        let directory = PathBuf::from(args.next().context("monitor directory missing")?);
        let build_id = args
            .next()
            .context("monitor build ID missing")?
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF8 build ID"))?;
        let mut server = minidumper::Server::with_name(minidumper::SocketName::path(
            socket.to_str().context("non-UTF8 crash socket path")?,
        ))?;
        server.run(
            Box::new(Handler {
                directory,
                build_id,
                panic: std::sync::Mutex::new(None),
                #[cfg(target_os = "macos")]
                snapshot: AtomicBool::new(false),
            }),
            &AtomicBool::new(false),
            None,
        )?;
        let _ = std::fs::remove_file(socket);
        Ok(true)
    }
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
pub use native::Capture;

/// Runs the isolated dump worker when invoked with its private launch arguments.
/// Return from main immediately when this returns true.
pub fn monitor_from_args() -> Result<bool> {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    return native::monitor_from_args();
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    Ok(false)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
/// Native capture is available on desktop Windows, Linux and macOS.
pub struct Capture;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
impl Capture {
    /// Reports an unsupported platform without starting a worker.
    pub fn start(_directory: &Path, _build_id: &str) -> Result<Self> {
        bail!("ARA-EXP-CRASH-004: native capture requires Windows, Linux or macOS")
    }
}
