//! Opt-in startup spans shared by the installed GUI and its service.
use std::{
    fs::OpenOptions,
    io::Write,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Append one completed span when ARA_HUB_STARTUP_TRACE names a diagnostic log.
/// Disabled during normal launches; never records paths, tokens or registry contents.
pub fn record(phase: &str, elapsed: Duration) {
    let Some(path) = std::env::var_os("ARA_HUB_STARTUP_TRACE") else {
        return;
    };
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let entry = serde_json::json!({"pid":std::process::id(),"phase":phase,"ms":elapsed.as_secs_f64()*1000.,"endUnixMs":unix_ms});
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{entry}");
    }
}
