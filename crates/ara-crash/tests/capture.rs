#![allow(clippy::unwrap_used)]
use ara_crash::{CrashKind, CrashReport, CrashSettings, UploadConsent};

#[test]
fn upload_requires_configuration_and_explicit_consent() {
    let dir = tempfile::tempdir().unwrap();
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
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_ara-crash"))
            .args(["probe", mode])
            .arg(dir.path())
            .spawn()
            .unwrap();
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
        // The monitor ends with its process and removes its `ara-<pid>-*.sock`.
        let socket = format!("ara-{}-", child.id());
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::read_dir(std::env::temp_dir())
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
