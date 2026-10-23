use anyhow::{Result, ensure};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// Exported crash policy. Configuration alone never authorizes a transmission.
#[derive(Debug, Clone, Default, Schema, Serialize, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrashSettings {
    /// Optional HTTPS receiver for explicit user-approved uploads; absent by default.
    #[serde(default)]
    pub upload_endpoint: Option<String>,
    /// Text shown by the game before asking the user to upload a crash.
    #[serde(default)]
    pub consent_text: String,
}
impl CrashSettings {
    /// Rejects an insecure receiver or a receiver without disclosure text.
    pub fn validate(&self) -> Result<()> {
        if let Some(url) = &self.upload_endpoint {
            ensure!(
                url.starts_with("https://")
                    && !url.contains(['\r', '\n'])
                    && !self.consent_text.trim().is_empty(),
                "ARA-EXP-CRASH-001: HTTPS upload endpoint requires consent text"
            );
        }
        Ok(())
    }
}

/// Origin of a locally recorded fault.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CrashKind {
    /// Native exception/signal captured by the monitor.
    Native,
    /// Rust panic, including source location and a forced backtrace.
    Panic,
    /// Unhandled managed exception with its managed stack.
    Managed,
}

/// A portable report; native reports keep the minidump alongside this file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrashReport {
    /// Versioned report contract.
    pub format: String,
    /// Export build identity; selects the immutable symbol archive.
    pub build_id: String,
    /// Fault source.
    pub kind: CrashKind,
    /// Panic/exception message or native exception description.
    pub message: String,
    /// Managed/Rust stack or the symbolized native stack.
    pub stack: String,
}
impl CrashReport {
    /// Creates a local record, without reading memory beyond the supplied stack.
    pub fn new(build_id: &str, kind: CrashKind, message: String, stack: String) -> Self {
        Self {
            format: "ara.crash/1".into(),
            build_id: build_id.into(),
            kind,
            message,
            stack,
        }
    }
    /// Structured evidence for the diagnostic bus and `ara why` crash explainer.
    pub fn diagnostic(&self) -> ara_diag::Diagnostic {
        ara_diag::Diagnostic::error(
            crate::CRASH,
            format!("{:?} crash: {}", self.kind, self.message),
        )
        .data(serde_json::json!({"crash":self}))
    }
}

/// Writes a new, synced local report. Existing artifacts are never overwritten.
pub fn write_report(directory: &Path, report: &CrashReport) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = directory.join(format!("{}-{stamp}.crash.json", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(crate::canonical(report)?.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}

/// Explicit user decision associated with one upload attempt.
pub enum UploadConsent {
    /// The player has accepted the configured consent text for this report.
    Granted,
    /// Declining never starts a network process.
    Declined,
}

/// Uploads a report/dump using the OS curl tool, only after explicit consent.
/// No upload is attempted by the crash handler or by player startup.
pub fn upload(settings: &CrashSettings, consent: UploadConsent, artifact: &Path) -> Result<()> {
    settings.validate()?;
    ensure!(
        matches!(consent, UploadConsent::Granted),
        "ARA-EXP-CRASH-002: crash upload declined"
    );
    let endpoint = settings
        .upload_endpoint
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("ARA-EXP-CRASH-002: uploads are disabled"))?;
    ensure!(
        artifact.is_file() && std::fs::metadata(artifact)?.len() <= 128 * 1024 * 1024,
        "ARA-EXP-CRASH-003: invalid or oversized crash upload"
    );
    let status = std::process::Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            "30",
            "--header",
            "Content-Type: application/octet-stream",
            "--data-binary",
        ])
        .arg(format!("@{}", artifact.display()))
        .arg(endpoint)
        .status()?;
    ensure!(status.success(), "ARA-EXP-CRASH-003: crash upload failed");
    Ok(())
}
