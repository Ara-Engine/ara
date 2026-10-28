//! Opt-in cross-process timing; the ordinary idle frame path never calls this.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Unix microseconds for comparing compiler, watcher and native-host events.
pub fn timestamp_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// Write an isolated event when ARA_HOTPATCH_TRACE_DIR is set. Each event owns its
/// file, so concurrent rustc/host processes need no shared lock or append protocol.
pub fn trace_event(event: &str, details: serde_json::Value) {
    let Some(directory) = std::env::var_os("ARA_HOTPATCH_TRACE_DIR") else {
        return;
    };
    let at = timestamp_us();
    let pid = std::process::id();
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::path::PathBuf::from(directory);
    let result = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(&path)?;
        std::fs::write(
            path.join(format!("{at}-{pid}-{seq}.json")),
            serde_json::to_vec(
                &serde_json::json!({"event":event,"at_us":at,"pid":pid,"details":details}),
            )?,
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Rust hotpatch timing output failed: {error:#}");
    }
}
