//! Exercise the shipped scripts in Windows PowerShell 5.1, without real services.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::{fs, os::windows::process::CommandExt, path::Path, process::Command};

#[test]
fn install_and_uninstall_accept_verbatim_paths_and_literal_directory_names() {
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = scripts.join("../../target/release-powershell-tests");
    fs::create_dir_all(&target).unwrap();
    let root = tempfile::tempdir_in(&target).unwrap();
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(scripts.join("tests/powershell_paths.ps1"))
        .arg("-Root")
        .arg(std::path::absolute(root.path()).unwrap())
        .arg("-Release")
        .arg(scripts.join("../../release"))
        .arg("-HubInstall")
        .arg(scripts.join("hub-install.ps1"))
        .env("ARA_CONFIG_DIR", root.path().join("config"))
        .env("ARA_CRASH_DIR", root.path().join("crashes"))
        .env("LOCALAPPDATA", root.path().join("local"))
        .env("APPDATA", root.path().join("roaming"))
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
