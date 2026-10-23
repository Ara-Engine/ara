//! Setup failures must reach testers as coded diagnostics with an actionable hint.
use std::{path::PathBuf, process::Command};

fn probe() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ara-player"));
    let output =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/runtime-polish/prerequisites");
    command
        .args(["--owner", "1"])
        .arg("--gpu-cache")
        .arg(output.join("gpu"))
        .env("ARA_PLAY_TOKEN", "setup-probe")
        .env("ARA_CRASH_DIR", output.join("crashes"))
        .env_remove("ARA_PLAY_SCRIPTS");
    command
}

#[test]
fn missing_vulkan_driver_has_a_doctor_hint() {
    let output = probe()
        .arg("--window")
        .env("VK_DRIVER_FILES", "Z:/ara-missing-vulkan-driver.json")
        .env("VK_ICD_FILENAMES", "Z:/ara-missing-vulkan-driver.json")
        .output()
        .expect("launch setup probe");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{error}");
    assert!(error.contains("ARA_GPU_BACKEND"), "{error}");
    assert!(error.contains("ara doctor"), "{error}");
}

#[test]
fn missing_dotnet_host_has_a_doctor_hint() {
    let settings = serde_json::json!({
        "hostDir":"Z:/ara-missing-dotnet-host",
        "compileServer":null,
        "modules":[]
    });
    let output = probe()
        .env("ARA_PLAY_SCRIPTS", settings.to_string())
        .output()
        .expect("launch setup probe");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{error}");
    assert!(error.contains("ARA0922"), "{error}");
    assert!(error.contains("ara doctor"), "{error}");
}
