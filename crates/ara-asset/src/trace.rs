//! Opt-in import traces; no trace data enters action keys or cooked bytes.
use serde_json::json;
use std::{
    fs::File,
    io::Write,
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

fn sink() -> Option<&'static Mutex<File>> {
    static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let directory = std::env::var_os("ARA_IMPORT_TRACE_DIR")?;
        std::fs::create_dir_all(&directory).ok()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(
                std::path::PathBuf::from(directory)
                    .join(format!("trace-{}.jsonl", std::process::id())),
            )
            .ok()?;
        Some(Mutex::new(file))
    })
    .as_ref()
}

pub(crate) fn enabled() -> bool {
    sink().is_some()
}

pub(crate) fn event(stage: &str, subject: &str, data: serde_json::Value) {
    let Some(sink) = sink() else { return };
    let value = json!({"stage":stage,"subject":subject,"pid":std::process::id(),
        "thread":format!("{:?}",std::thread::current().id()),
        "unixUs":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_micros() as u64,
        "processCpuMs":process_cpu_ms(),"data":data});
    let mut line = ara_json::to_canonical_string(&ara_json::sort_keys(&value));
    line.push('\n');
    let _ = sink
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .write_all(line.as_bytes());
}

#[cfg(windows)]
fn process_cpu_ms() -> Option<f64> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let mut times = [FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    }; 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: The pseudo handle is valid and the four output pointers reference
    // distinct initialized FILETIME values for this synchronous call.
    if unsafe { GetProcessTimes(GetCurrentProcess(), created, exited, kernel, user) } == 0 {
        return None;
    }
    let ticks = |t: &FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Some((ticks(kernel) + ticks(user)) as f64 / 10_000.0)
}

#[cfg(not(windows))]
fn process_cpu_ms() -> Option<f64> {
    None
}

#[cfg(windows)]
pub(crate) fn peak_rss_bytes() -> Option<usize> {
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    counters.cb = size;
    // SAFETY: This process handle is valid; counters points to an initialized
    // structure of the declared size and remains live for the synchronous call.
    (unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } != 0)
        .then_some(counters.PeakWorkingSetSize)
}

#[cfg(not(windows))]
pub(crate) fn peak_rss_bytes() -> Option<usize> {
    None
}

pub(crate) struct Scope(Option<(&'static str, String, Instant, Option<usize>)>);
impl Scope {
    pub(crate) fn new(stage: &'static str, subject: &str) -> Self {
        Self(enabled().then(|| (stage, subject.to_owned(), Instant::now(), None)))
    }
    pub(crate) fn bytes(mut self, bytes: usize) -> Self {
        if let Some(state) = &mut self.0 {
            state.3 = Some(bytes);
        }
        self
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some((stage, subject, start, bytes)) = &self.0 {
            event(
                stage,
                subject,
                json!({"elapsedMs":start.elapsed().as_secs_f64()*1000.0,"bytes":bytes}),
            );
        }
    }
}
