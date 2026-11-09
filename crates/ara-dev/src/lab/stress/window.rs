//! Overnight coordination lock, independent of ara-dev's OS-backed GPU queue.
use anyhow::{Result, ensure};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn lock_path() -> PathBuf {
    std::env::var_os("ARA_RENDER_V3_QUIET_LOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| "D:/AraBuild/render-v3/quiet-window.lock".into())
}

/// Retain pmon availability and utilization before/after the guarded window.
pub fn telemetry(path: &std::path::Path) -> Result<()> {
    let mut log = fs::File::create(path)?;
    for args in [
        vec!["pmon", "-c", "1"],
        vec![
            "--query-gpu=name,utilization.gpu,memory.used,temperature.gpu",
            "--format=csv",
        ],
    ] {
        let output = std::process::Command::new("nvidia-smi")
            .args(&args)
            .output();
        writeln!(log, "nvidia-smi {args:?}")?;
        match output {
            Ok(output) => {
                writeln!(log, "status {}", output.status)?;
                log.write_all(&output.stdout)?;
                log.write_all(&output.stderr)?;
            }
            Err(error) => writeln!(log, "n/a: {error}")?,
        }
    }
    Ok(())
}

pub fn check_foreign() -> Result<()> {
    let path = lock_path();
    if path.exists() {
        anyhow::bail!(
            "quiet window exists: {}: {}. Retry after its owner releases it; stale-lock expiry must be checked by the coordinator",
            path.display(),
            fs::read_to_string(&path)?
        );
    }
    Ok(())
}
pub struct QuietWindow {
    path: PathBuf,
    token: String,
}
impl QuietWindow {
    pub fn acquire() -> Result<Self> {
        check_foreign()?;
        let path = lock_path();
        let last = path.with_extension("t1-last-end");
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        if let Ok(previous) = fs::read_to_string(&last)
            && let Ok(previous) = previous.trim().parse::<u64>()
        {
            ensure!(
                now.saturating_sub(previous) >= 1200,
                "T1 must leave 20 minutes between quiet windows; use --quality-only or --report-only meanwhile"
            );
        }
        let stamps = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "$now=Get-Date; $now.ToString('o'); $now.AddMinutes(44).ToString('o')",
            ])
            .output()?;
        ensure!(
            stamps.status.success(),
            "cannot timestamp overnight quiet lease"
        );
        let stamps = String::from_utf8(stamps.stdout)?;
        let mut lines = stamps.lines();
        let start = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing lease start"))?;
        let expires = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing lease expiry"))?;
        let token = format!(
            "owner=T1-{} start={start} expires={expires}\n",
            std::process::id()
        );
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        Ok(Self { path, token })
    }
}
impl Drop for QuietWindow {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).ok().as_deref() == Some(&self.token) {
            let _ = fs::remove_file(&self.path);
            if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                let _ = fs::write(
                    self.path.with_extension("t1-last-end"),
                    now.as_secs().to_string(),
                );
            }
        }
    }
}
