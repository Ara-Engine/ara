//! One host owns patch publication; websocket workers only prepare deliveries.
use crate::Contract;
use anyhow::{Context, Result};
use ara_diag::{Code, Diagnostic};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

static GENERATION: AtomicU64 = AtomicU64::new(0);
#[repr(C)]
struct Activity {
    count: AtomicUsize,
    marker: [u8; 24],
}
// Keep the export marker attached to storage used by every hot call, even if
// Cargo feature unification enables dispatch without enabling a host's connect().
// SAFETY: This name belongs only to Ara's single opt-in runtime definition.
#[unsafe(no_mangle)]
#[used]
static ARA_HOTPATCH_ACTIVITY: Activity = Activity {
    count: AtomicUsize::new(0),
    marker: *b"ARA_DEV_RUST_HOTPATCH_v1",
};
const WRITING: usize = usize::MAX;
const BUILD_MARKER: &str = "ARA_DEV_RUST_HOTPATCH_v1";

/// One catalog owner for code-identity warnings from the runtime and simulation.
pub const DETERMINISM: Code = Code(7603);

/// Process-wide applied generation. Any nonzero value taints every simulation session.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Take the driver's native argument envelope, preserving spaces that dx 0.7.10 splits.
/// Clearing it prevents helper/player children inheriting their parent's arguments.
///
/// # Safety
/// Call only at process startup, before creating any threads or native runtimes.
pub unsafe fn args_os() -> Result<Vec<std::ffi::OsString>> {
    let Some(text) = std::env::var_os("ARA_HOTPATCH_ARGS") else {
        return Ok(std::env::args_os().collect());
    };
    // SAFETY: The caller guarantees single-threaded process startup.
    unsafe { std::env::remove_var("ARA_HOTPATCH_ARGS") };
    let args: Vec<String> =
        serde_json::from_str(text.to_str().context("invalid Rust hotpatch arguments")?)?;
    let mut native = vec![std::env::current_exe()?.into_os_string()];
    native.extend(args.into_iter().map(std::ffi::OsString::from));
    Ok(native)
}

/// Pins synchronous hot calls, including parallel ECS work, against table publication.
/// A host must drop all leases before calling its patch boundary. No GPU wait is needed:
/// patches retain resources; ordinary GPU retirement still follows queue timelines.
pub struct Lease {
    _private: (),
}
impl Lease {
    /// Enter a hot call. Hosts publish only after all concurrent recording scopes finish.
    pub fn enter() -> Self {
        loop {
            let count = ARA_HOTPATCH_ACTIVITY.count.load(Ordering::Acquire);
            if count != WRITING
                && ARA_HOTPATCH_ACTIVITY
                    .count
                    .compare_exchange_weak(count, count + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                return Self { _private: () };
            }
            std::hint::spin_loop();
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        ARA_HOTPATCH_ACTIVITY.count.fetch_sub(1, Ordering::Release);
    }
}
struct Publish;
impl Publish {
    fn try_enter() -> Option<Self> {
        ARA_HOTPATCH_ACTIVITY
            .count
            .compare_exchange(0, WRITING, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Publish {
    fn drop(&mut self) {
        ARA_HOTPATCH_ACTIVITY.count.store(0, Ordering::Release);
    }
}

/// Development status suitable for diagnostics, the lab and the editor activity feed.
#[derive(Default, Debug, Serialize)]
pub struct Status {
    /// Count of patches actually published to the running host.
    pub generation: u64,
    /// Unix milliseconds of the last successful publication, never simulation time.
    pub last_patch_ms: Option<u64>,
    /// Last rejected patch's reason; a successful publication clears it.
    pub last_error: Option<String>,
}

enum Delivery {
    Ready(subsecond::JumpTable, u64),
    Rejected(String),
}

/// The sole process patch owner. Construct at host startup, poll between frames.
/// Without dx it stays disconnected and its boundary is a single atomic load.
pub struct Hotpatch {
    receiver: mpsc::Receiver<Delivery>,
    pending: Arc<AtomicBool>,
    status: Status,
}

impl Hotpatch {
    /// Connect to the local dx server, using the ara-dev source contract.
    /// A missing contract disables patch acceptance instead of bypassing layout checks.
    pub fn connect() -> Self {
        // Keep an unmistakable marker in every hotpatch host for the export guard.
        eprintln!("{BUILD_MARKER}: experimental Rust hotpatch host");
        let (sender, receiver) = mpsc::channel();
        let pending = Arc::new(AtomicBool::new(false));
        let endpoint = dioxus_cli_config::devserver_ws_endpoint().filter(|_| {
            if let (Ok(executable), Some(expected)) = (
                std::env::current_exe(), std::env::var_os("ARA_HOTPATCH_HOST_BIN"),
            ) && !host_matches(&executable, Some(&expected)) {
                eprintln!("Rust hotpatch: this companion keeps its built code; use its own player driver for Rust iteration");
                return false;
            }
            true
        });
        if let Some(endpoint) = endpoint {
            #[cfg(windows)]
            {
                // SAFETY: This changes only our host's priority, using its always-valid
                // pseudo handle. Cargo/rustc keep the patch driver's BelowNormal class.
                let result = unsafe {
                    use windows_sys::Win32::System::Threading::*;
                    SetPriorityClass(GetCurrentProcess(), NORMAL_PRIORITY_CLASS)
                };
                if result == 0 {
                    Diagnostic::warning(
                        Code(7600),
                        "Could not restore native host priority above its background compiler",
                    )
                    .emit();
                }
            }
            let ready = Arc::clone(&pending);
            let worker = std::thread::Builder::new()
                .name("ara-rust-patches".into())
                .spawn(move || {
                    if let Err(error) = receive(&endpoint, &sender, &ready) {
                        let _ = sender.send(Delivery::Rejected(error.to_string()));
                        ready.store(true, Ordering::Release);
                    }
                });
            if let Err(error) = worker {
                Diagnostic::error(
                    Code(7600),
                    format!("Rust patch receiver could not start: {error}"),
                )
                .emit();
            }
        }
        Self {
            receiver,
            pending,
            status: Status::default(),
        }
    }

    /// Publish only when no hot system or pass is executing. Returns true after an apply.
    /// A busy renderer defers to the next boundary; it never blocks the human's frame.
    pub fn frame_boundary(&mut self) -> bool {
        if !self.pending.load(Ordering::Acquire) {
            return false;
        }
        let Some(_publish) = Publish::try_enter() else {
            return false;
        };
        self.pending.store(false, Ordering::Release);
        let mut applied = false;
        while let Ok(delivery) = self.receiver.try_recv() {
            match delivery {
                Delivery::Rejected(reason) => self.reject(reason),
                Delivery::Ready(table, compile_ms) => {
                    // SAFETY: Only our local dx connection supplies tables, checked for this
                    // PID/build and body-only source compatibility before delivery. All hot
                    // calls are quiescent under Publish. Subsecond retains old code libraries;
                    // no live object, vtable, resource layout or GPU handle is replaced here.
                    match unsafe { subsecond::apply_patch(table) } {
                        Ok(()) => {
                            self.status.generation = GENERATION.fetch_add(1, Ordering::Release) + 1;
                            self.status.last_patch_ms = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|d| d.as_millis() as u64);
                            self.status.last_error = None;
                            Diagnostic::info(Code(7601), "Rust patch applied; simulation sessions are development-only")
                                .data(serde_json::json!({"generation":self.status.generation,"atMs":self.status.last_patch_ms,"compileMs":compile_ms})).emit();
                            Diagnostic::warning(DETERMINISM, "Rust code identity changed; replay and network continuation require a rebuild").emit();
                            println!(
                                "ARA7601 Rust patch applied: generation={} at_ms={:?} compile_ms={compile_ms}",
                                self.status.generation, self.status.last_patch_ms
                            );
                            applied = true;
                        }
                        Err(error) => self.reject(format!("needs rebuild: {error}")),
                    }
                }
            }
        }
        applied
    }

    /// Read the last apply/rejection state without polling transport or allocating.
    pub fn status(&self) -> &Status {
        &self.status
    }

    fn reject(&mut self, reason: String) {
        Diagnostic::warning(Code(7602), &reason).emit();
        eprintln!("ARA7602 Rust patch rejected: {reason}");
        self.status.last_error = Some(reason);
    }
}

#[derive(Deserialize)]
enum Message {
    HotReload(Reload),
    HotPatchStart,
    FullReloadFailed,
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
struct Reload {
    jump_table: Option<subsecond::JumpTable>,
    for_pid: Option<u32>,
    for_build_id: Option<u64>,
    ms_elapsed: u64,
}

fn receive(endpoint: &str, sender: &mpsc::Sender<Delivery>, ready: &AtomicBool) -> Result<()> {
    let executable = std::env::current_exe()?;
    let expected = std::env::var_os("ARA_HOTPATCH_HOST_BIN");
    anyhow::ensure!(
        host_matches(&executable, expected.as_deref()),
        "ARA7600: this helper/Play companion has a different patch baseline; launch its own ara-dev hotpatch player lane"
    );
    anyhow::ensure!(
        endpoint.starts_with("ws://127.0.0.1:")
            || endpoint.starts_with("ws://localhost:")
            || endpoint.starts_with("ws://[::1]:"),
        "Rust patch transport must be loopback"
    );
    let path = PathBuf::from(
        std::env::var_os("ARA_HOTPATCH_CONTRACT")
            .context("ARA7600: launch with ara-dev hotpatch; source contract is missing")?,
    );
    let contract = Contract::read(&path)?;
    let build = dioxus_cli_config::build_id();
    let uri = format!(
        "{endpoint}?aslr_reference={}&build_id={build}&pid={}",
        subsecond::aslr_reference(),
        std::process::id()
    );
    let (mut socket, _) = tungstenite::connect(uri)?;
    while let Ok(message) = socket.read() {
        let tungstenite::Message::Text(text) = message else {
            continue;
        };
        let reload = match serde_json::from_str(&text) {
            Ok(Message::HotReload(reload)) => reload,
            Ok(Message::HotPatchStart) => {
                if let Err(error) = contract.validate() {
                    if sender.send(Delivery::Rejected(error.to_string())).is_err() {
                        return Ok(());
                    }
                    ready.store(true, Ordering::Release);
                }
                continue;
            }
            Ok(Message::FullReloadFailed) => {
                let reason = contract.validate().err().map_or_else(
                    || {
                        "Rust compilation failed; last-good code retained (see ara-dev output)"
                            .into()
                    },
                    |e| e.to_string(),
                );
                if sender.send(Delivery::Rejected(reason)).is_err() {
                    return Ok(());
                }
                ready.store(true, Ordering::Release);
                continue;
            }
            _ => continue,
        };
        if reload.for_pid != Some(std::process::id()) || reload.for_build_id != Some(build) {
            continue;
        }
        let Some(table) = reload.jump_table else {
            continue;
        };
        let delivery = match contract.validate() {
            Ok(()) => Delivery::Ready(table, reload.ms_elapsed),
            Err(error) => Delivery::Rejected(error.to_string()),
        };
        if sender.send(delivery).is_err() {
            return Ok(());
        }
        ready.store(true, Ordering::Release);
    }
    Ok(())
}

fn host_matches(executable: &std::path::Path, expected: Option<&std::ffi::OsStr>) -> bool {
    let Some((stem, expected)) = executable
        .file_stem()
        .and_then(|n| n.to_str())
        .zip(expected.and_then(|n| n.to_str()))
    else {
        return false;
    };
    // DX retains Cargo's artifact hash in the bundled native filename. Helpers
    // still need the exact bin name; an arbitrary prefix match is not sufficient.
    stem == expected
        || stem
            .strip_prefix(expected)
            .and_then(|s| s.strip_prefix('-'))
            .is_some_and(|suffix| {
                (8..=64).contains(&suffix.len()) && suffix.bytes().all(|b| b.is_ascii_hexdigit())
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_identity_accepts_cargo_hashes_and_refuses_other_companions() {
        let expected = Some(std::ffi::OsStr::new("ara-editor-hotpatch"));
        assert!(host_matches(
            std::path::Path::new("ara-editor-hotpatch.exe"),
            expected
        ));
        assert!(host_matches(
            std::path::Path::new("ara-editor-hotpatch-e8e1de04.exe"),
            expected
        ));
        assert!(!host_matches(
            std::path::Path::new("ara-player-e8e1de04.exe"),
            expected
        ));
        assert!(!host_matches(
            std::path::Path::new("ara-editor-hotpatch-helper.exe"),
            expected
        ));
        assert!(!host_matches(
            std::path::Path::new("ara-editor-hotpatch.exe"),
            None
        ));
    }
    #[test]
    fn publication_defers_until_all_hot_work_finishes() {
        let lease = Lease::enter();
        assert!(Publish::try_enter().is_none());
        drop(lease);
        let writer = Publish::try_enter();
        assert!(writer.is_some());
        drop(writer);
        let _lease = Lease::enter();
    }
}
