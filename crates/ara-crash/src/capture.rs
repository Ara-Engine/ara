use anyhow::{Context, Result, bail};
use std::path::Path;

#[cfg(any(windows, target_os = "linux"))]
mod native {
    use super::*;
    use std::{
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    static ATTACHED: AtomicBool = AtomicBool::new(false);
    type PanicHook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static;

    /// Owns the native crash handler and the monitor child. Drop detaches and reaps.
    pub struct Capture {
        handler: Option<crash_handler::CrashHandler>,
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
            // SAFETY: the callback owns the connected client for the handler's lifetime.
            // It performs only minidumper's signal-safe IPC; all allocation and I/O
            // for the dump happens in the monitor process.
            let event = unsafe {
                crash_handler::make_crash_event(move |context: &crash_handler::CrashContext| {
                    crash_handler::CrashEventResult::Handled(client.request_dump(context).is_ok())
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
            let previous_hook: Arc<PanicHook> = Arc::from(std::panic::take_hook());
            let previous = previous_hook.clone();
            let build = build_id.to_owned();
            std::panic::set_hook(Box::new(move |info| {
                let report = crate::CrashReport::new(
                    &build,
                    crate::CrashKind::Panic,
                    info.to_string(),
                    std::backtrace::Backtrace::force_capture().to_string(),
                );
                let _ = crate::write_report(&directory, &report);
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
    }
    impl minidumper::ServerHandler for Handler {
        fn create_minidump_file(&self) -> std::io::Result<(std::fs::File, PathBuf)> {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(std::io::Error::other)?
                .as_nanos();
            let path = self
                .directory
                .join(format!("{}-{stamp}.dmp", std::process::id()));
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
                if let Ok(json) = crate::canonical(&report) {
                    let _ = std::fs::write(dump.path.with_extension("crash.json"), json);
                }
            }
            minidumper::LoopAction::Exit
        }
        fn on_message(&self, _kind: u32, _buffer: Vec<u8>) {}
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
            }),
            &AtomicBool::new(false),
            None,
        )?;
        let _ = std::fs::remove_file(socket);
        Ok(true)
    }
}

#[cfg(any(windows, target_os = "linux"))]
pub use native::Capture;

/// Runs the isolated dump worker when invoked with its private launch arguments.
/// Return from main immediately when this returns true.
pub fn monitor_from_args() -> Result<bool> {
    #[cfg(any(windows, target_os = "linux"))]
    return native::monitor_from_args();
    #[cfg(not(any(windows, target_os = "linux")))]
    Ok(false)
}

#[cfg(not(any(windows, target_os = "linux")))]
/// Native capture is available on desktop Windows and Linux.
pub struct Capture;
#[cfg(not(any(windows, target_os = "linux")))]
impl Capture {
    /// Reports an unsupported platform without starting a worker.
    pub fn start(_directory: &Path, _build_id: &str) -> Result<Self> {
        bail!("ARA-EXP-CRASH-004: native capture requires Windows or Linux")
    }
}
