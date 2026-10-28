#![allow(clippy::unwrap_used)]
use ara_crash::{CrashKind, CrashReport, CrashSettings, UploadConsent};

fn isolated_directory() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/crash-ipc");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(std::path::absolute(root).unwrap()).unwrap()
}

#[cfg(feature = "symbols")]
#[test]
fn monitor_becomes_ready_with_a_long_temp_directory() {
    let dir = isolated_directory();
    let temp = dir.path().join("long-temp-".repeat(12));
    std::fs::create_dir_all(&temp).unwrap();
    assert!(temp.to_str().unwrap().len() > 160);
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ara-crash"));
    command
        .args(["probe", "ready"])
        .arg(dir.path().join("crashes"))
        .env("TEMP", &temp)
        .env("TMP", &temp)
        .env("TMPDIR", &temp)
        .env("LOCALAPPDATA", dir.path())
        .env("ARA_CRASH_IPC_DIR", dir.path());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("crash monitor ready"));
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("ara-c-")
    }));

    #[cfg(windows)]
    {
        // Use the normal per-user base with a deep TEMP. Keep LOCALAPPDATA under
        // target, and leave SystemRoot intact because Winsock loads from it.
        let local = dir.path().parent().unwrap();
        let output = command
            .env_remove("ARA_CRASH_IPC_DIR")
            .env("LOCALAPPDATA", local)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            std::fs::read_dir(local.join("Ara/ipc"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[cfg(feature = "symbols")]
#[test]
fn monitor_rejects_overlong_paths_with_a_coded_byte_count() {
    let dir = isolated_directory();
    let socket = dir.path().join("é".repeat(82));
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ara-crash"));
    command
        .arg("--ara-crash-monitor")
        .arg(&socket)
        .arg(dir.path())
        .arg("guard-probe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().unwrap();
    assert!(!output.status.success());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("ARA-EXP-CRASH-004"), "{diagnostic}");
    assert!(
        diagnostic.contains(&format!(
            "socket path too long: {} bytes",
            socket.to_str().unwrap().len()
        )),
        "{diagnostic}"
    );
    assert!(!socket.exists());
}

#[test]
fn upload_requires_configuration_and_explicit_consent() {
    let dir = isolated_directory();
    let report = CrashReport::new("build", CrashKind::Panic, "message".into(), "stack".into());
    let file = ara_crash::write_report(dir.path(), &report).unwrap();
    assert!(ara_crash::upload(&CrashSettings::default(), UploadConsent::Granted, &file).is_err());
    let settings = CrashSettings {
        upload_endpoint: Some("https://example.invalid/crashes".into()),
        consent_text: "Send this diagnostic and dump, which may contain personal data?".into(),
    };
    assert!(ara_crash::upload(&settings, UploadConsent::Declined, &file).is_err());
    assert!(
        CrashSettings {
            upload_endpoint: Some("http://example.invalid".into()),
            ..settings.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        CrashSettings {
            consent_text: String::new(),
            ..settings
        }
        .validate()
        .is_err()
    );
    let decoded: CrashReport = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(decoded.diagnostic().code, ara_crash::CRASH);
}

#[cfg(all(
    feature = "symbols",
    any(windows, target_os = "linux", target_os = "macos")
))]
#[test]
fn native_crash_and_panic_are_captured_in_separate_processes() {
    use std::{
        process::Command,
        time::{Duration, Instant},
    };
    for mode in ["native", "panic", "recovered"] {
        let dir = isolated_directory();
        let mut command = Command::new(env!("CARGO_BIN_EXE_ara-crash"));
        command
            .args(["probe", mode])
            .arg(dir.path())
            .env("ARA_CRASH_IPC_DIR", dir.path());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("crash probe timed out");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!status.success());
        // The monitor ends with its process and removes its private IPC directory.
        let socket = format!("ara-c-{}-", child.id());
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&socket))
        {
            assert!(
                Instant::now() < deadline,
                "the crash monitor outlived its {mode} probe"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let artifacts = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        let artifact = artifacts
            .iter()
            .find(|p| {
                p.extension()
                    .is_some_and(|ext| ext == if mode == "panic" { "json" } else { "dmp" })
            })
            .expect("crash artifact");
        if mode != "panic" {
            let dump = minidump::Minidump::read_path(artifact).unwrap();
            assert!(dump.get_stream::<minidump::MinidumpException>().is_ok());
            if mode == "recovered" {
                assert_eq!(
                    artifacts
                        .iter()
                        .filter(|p| p.extension().is_some_and(|ext| ext == "dmp"))
                        .count(),
                    2
                );
            }
        } else {
            let report = ara_crash::symbols::symbolize(artifact, dir.path()).unwrap();
            assert!(!report.stack.is_empty());
            assert!(
                artifacts
                    .iter()
                    .any(|p| p.extension().is_some_and(|ext| ext == "dmp"))
            );
            assert!(report.message.contains("Ara panic capture probe"));
        }
    }
}
