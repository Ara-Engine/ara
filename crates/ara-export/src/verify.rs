//! Final export verification. Static checks are explicitly distinguished from a
//! smoke launch; missing renderer/device hooks are reported, never synthesized.
use crate::{
    ExportPreset,
    tools::{self, Toolchain},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, process::Command, time::Duration};

/// Verification evidence persisted in report.json for every completed lane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    /// `desktop_smoke`, `static_elf`, `android_structure` or `android_device`.
    pub method: String,
    /// Whether the checks selected by method passed.
    pub passed: bool,
    /// True only if the exported program actually executed.
    pub runtime_executed: bool,
    /// Log file relative to the export directory.
    pub logs: String,
    /// Captured PNG path when a renderer/device supplied one.
    pub screenshot: Option<String>,
    /// Runtime CPU/GPU measurements, or null when only static checks ran.
    pub perf: Value,
    /// Raw runtime conformance result / binary audit evidence.
    pub evidence: Value,
    /// Missing hooks/devices, with explicit reasons.
    pub unavailable: Vec<String>,
}

/// Parses the runtime's single JSON result line, rejecting missing/failed checks.
pub fn runtime_result(output: &str) -> Result<Value> {
    let line = output
        .lines()
        .find_map(|l| l.split_once("ARA_RESULT ").map(|(_, v)| v))
        .context("ARA-EXP-VER-001: runtime emitted no ARA_RESULT")?;
    let value: Value = serde_json::from_str(line)?;
    ensure!(
        value.get("verified").and_then(Value::as_bool) == Some(true),
        "ARA-EXP-VER-002: runtime verification failed"
    );
    if let Some(fault) = value.get("fault_contained") {
        ensure!(
            fault.as_bool() == Some(true),
            "ARA-EXP-VER-003: script exception containment failed"
        );
    }
    Ok(value)
}

/// Launches the headless desktop player with a 30-second deadline.
pub fn desktop(dist: &Path) -> Result<VerifyReport> {
    let log = dist.join("verify.log");
    let output = tools::run(
        Command::new(dist.join(tools::executable("ara-player")))
            .arg("--headless")
            .current_dir(dist),
        &log,
        Duration::from_secs(30),
    )?;
    let result = runtime_result(&output)?;
    let launch: Value = serde_json::from_slice(&std::fs::read(dist.join("ara-launch.json"))?)?;
    if let Some(contract) = launch.get("contract") {
        ensure!(
            result.get("template_contract") == Some(contract),
            "ARA-EXP-TPL-005: player result does not match the selected template contract"
        );
    }
    Ok(VerifyReport { method:"desktop_smoke".into(),passed:true,runtime_executed:true,logs:"verify.log".into(),screenshot:None,perf:result.get("tick").cloned().unwrap_or(Value::Null),evidence:result,unavailable:vec!["ARA-EXP-BRG-010: renderer capture hook not installed in the headless conformance template".into(),"ARA-EXP-BRG-015: GPU timing hook requires ara-render".into()] })
}

/// Audits all exported ELF images against glibc 2.31 without starting WSL.
pub fn linux(dist: &Path) -> Result<VerifyReport> {
    fn walk(path: &Path, evidence: &mut Vec<Value>) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, evidence)?;
                continue;
            }
            let bytes = std::fs::read(&path)?;
            if !bytes.starts_with(b"\x7fELF") {
                continue;
            }
            crate::android::validate_elf(&bytes, 62, 4096)?;
            let mut versions = Vec::new();
            for token in bytes.split(|b| *b == 0) {
                if let Some(suffix) = token.strip_prefix(b"GLIBC_") {
                    let text = std::str::from_utf8(suffix).unwrap_or("");
                    if text.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
                        let version: Vec<u32> =
                            text.split('.').filter_map(|v| v.parse().ok()).collect();
                        ensure!(
                            version.as_slice() <= [2, 31].as_slice(),
                            "ARA-EXP-LNX-010: {} requires GLIBC_{text}, baseline is 2.31",
                            path.display()
                        );
                        versions.push(text.to_owned());
                    }
                }
            }
            evidence.push(json!({"file":path.file_name().map(|s|s.to_string_lossy()),"glibc_versions":versions}));
        }
        Ok(())
    }
    let mut evidence = Vec::new();
    walk(dist, &mut evidence)?;
    ensure!(
        !evidence.is_empty(),
        "ARA-EXP-LNX-105: no ELF files exported"
    );
    tools::write_json(&dist.join("verify.log"), &evidence)?;
    Ok(VerifyReport {method:"static_elf".into(),passed:true,runtime_executed:false,logs:"verify.log".into(),screenshot:None,perf:Value::Null,evidence:json!(evidence),unavailable:vec!["Linux smoke launch was not requested; static ELF architecture, load segments and glibc requirements verified without starting WSL".into()]})
}

/// Validates both Android packages and optionally installs on an explicitly
/// selected device. An emulator is never started by the exporter.
pub fn android(
    dist: &Path,
    preset: &ExportPreset,
    t: &Toolchain,
    serial: Option<&str>,
    allow_emulator: bool,
) -> Result<VerifyReport> {
    crate::android::validate_package(&dist.join("game.apk"), false)?;
    crate::android::validate_package(&dist.join("game.aab"), true)?;
    let Some(serial) = serial else {
        let evidence = json!({"abis":["arm64-v8a","x86_64"],"elf_alignment":16384,"zip_alignment":16384,"apk_signature":"verified","aab_signature":"verified","target_api":preset.target_api});
        tools::write_json(&dist.join("verify.log"), &evidence)?;
        return Ok(VerifyReport {method:"android_structure".into(),passed:true,runtime_executed:false,logs:"verify.log".into(),screenshot:None,perf:Value::Null,evidence,unavailable:vec!["ARA-EXP-AND-009: no device requested; pass --device SERIAL (and --allow-emulator for an existing emulator)".into()]});
    };
    ensure!(
        !serial.starts_with("emulator-") || allow_emulator,
        "ARA-EXP-AND-029: emulator verification requires --allow-emulator"
    );
    let program = t.adb().context("ARA-EXP-AND-011: adb missing")?;
    let adb = ara_bridge::Adb::new(program.clone());
    ensure!(
        adb.list()?
            .iter()
            .any(|d| d.serial == serial && d.state == "device"),
        "ARA-EXP-AND-009: selected device is not authorized/connected"
    );
    let exec = |args: &[&str], name: &str| {
        tools::run(
            Command::new(&program).args(["-s", serial]).args(args),
            &dist.join(name),
            Duration::from_secs(30),
        )
    };
    tools::run(
        Command::new(&program)
            .args(["-s", serial, "install", "-r", "-t", "--no-incremental"])
            .arg(dist.join("game.apk")),
        &dist.join("install.log"),
        Duration::from_secs(60),
    )?;
    // Do not clear the user's global logcat ring; select this launch's process instead.
    exec(
        &["shell", "am", "force-stop", &preset.package],
        "force-stop.log",
    )?;
    exec(
        &[
            "shell",
            "am",
            "start",
            "-W",
            "-n",
            &format!("{}/android.app.NativeActivity", preset.package),
        ],
        "launch.log",
    )?;
    let pid = exec(&["shell", "pidof", &preset.package], "pid.log")?;
    let pid = pid.trim();
    ensure!(
        !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()),
        "ARA-EXP-VER-005: launched runtime has no live process"
    );
    let started = std::time::Instant::now();
    let output = loop {
        let log = exec(
            &[
                "logcat",
                "-d",
                "-v",
                "brief",
                "--pid",
                pid,
                "-s",
                "ara:*",
                "AndroidRuntime:E",
            ],
            "verify.log",
        )?;
        if log.contains("ARA_RESULT ") {
            break log;
        }
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "ARA-EXP-VER-004: device timed out waiting for ARA_RESULT"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    std::fs::write(dist.join("verify.log"), &output)?;
    let result = runtime_result(&output)?;
    let screenshot = adb.screenshot(serial)?;
    std::fs::write(dist.join("screenshot.png"), screenshot)?;
    exec(
        &["shell", "am", "force-stop", &preset.package],
        "force-stop.log",
    )?;
    Ok(VerifyReport {
        method: "android_device".into(),
        passed: true,
        runtime_executed: true,
        logs: "verify.log".into(),
        screenshot: Some("screenshot.png".into()),
        perf: result.get("tick").cloned().unwrap_or(Value::Null),
        evidence: result,
        unavailable: vec![],
    })
}
