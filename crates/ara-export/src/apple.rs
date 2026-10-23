//! Versioned Mac-agent seam. Windows prepares packs and requests a remote build;
//! Xcode, provisioning, codesign, notarization and device verification stay on Mac.
use crate::{ExportPreset, Platform, tools};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, process::Command, time::Duration};

/// Build request exchanged with `ara-export-agent --stdio` over authenticated SSH.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Always `ara.export.agent/1`.
    pub protocol: String,
    /// Export preset, including Apple silicon template pins.
    pub preset: ExportPreset,
    /// BLAKE3 identity of the pack available through artifact transfer.
    pub pack_hash: String,
    /// Content-addressed native/C# source archive ID, never a local Windows path.
    pub source_hash: String,
}

/// Mac capabilities response, validated before any build request is dispatched.
#[derive(Debug, Serialize, Deserialize)]
pub struct Capabilities {
    /// Protocol identifier.
    pub protocol: String,
    /// Must be `arm64` under D15.
    pub arch: String,
    /// Available Xcode SDKs, signing identities and template IDs.
    pub capabilities: Value,
}

/// Probes an explicitly configured SSH destination with batch authentication.
/// This does not initiate an export or modify the remote machine.
pub fn probe(destination: &str, log: &Path) -> Result<Capabilities> {
    ensure!(
        !destination.starts_with('-')
            && destination
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "@._-".contains(c)),
        "ARA-EXP-APL-003: invalid SSH destination"
    );
    let output = tools::run(
        Command::new("ssh").args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            destination,
            "ara-export-agent",
            "--capabilities",
        ]),
        log,
        Duration::from_secs(15),
    )?;
    let caps: Capabilities = serde_json::from_str(&output)?;
    ensure!(
        caps.protocol == "ara.export.agent/1" && caps.arch == "arm64",
        "ARA-EXP-APL-004: Mac agent must speak ara.export.agent/1 on arm64"
    );
    Ok(caps)
}

/// Explicit platform stub until the Mac session supplies build/sign/verify.
pub fn require_agent(preset: &ExportPreset) -> Result<()> {
    ensure!(
        matches!(preset.platform, Platform::Macos | Platform::Ios),
        "Apple seam used for a non-Apple lane"
    );
    if preset.mac_agent.is_none() {
        bail!(
            "ARA-EXP-APL-001: configure mac_agent in the preset; Apple builds require Xcode on the MacBook"
        );
    }
    bail!(
        "ARA-EXP-APL-002: Mac agent transport is ready for integration; implement ara-export-agent build/sign/verify in the Mac session (see crates/ara-export/README.md)"
    )
}
