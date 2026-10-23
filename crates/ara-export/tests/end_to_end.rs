//! Opt-in real toolchain exports. Run with ARA_EXPORT_E2E=1 after preparing pins.
#![allow(clippy::unwrap_used)]
use ara_export::{ExportPreset, ExportRequest};

#[test]
#[ignore = "requires platform templates and .NET/Android toolchains; see README"]
fn exports_sample_windows_linux_android_with_reports() {
    assert_eq!(std::env::var("ARA_EXPORT_E2E").as_deref(), Ok("1"));
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let presets = std::path::PathBuf::from(
        std::env::var_os("ARA_EXPORT_PRESETS").expect("ARA_EXPORT_PRESETS directory"),
    );
    let output = tempfile::tempdir().unwrap();
    let artifacts = std::env::var_os("ARA_EXPORT_ARTIFACTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| output.path().to_path_buf());
    std::fs::create_dir_all(&artifacts).unwrap();
    for platform in ["windows", "linux", "android"] {
        let preset = ExportPreset::from_text(
            &std::fs::read_to_string(presets.join(format!("{platform}.export.json"))).unwrap(),
        )
        .unwrap();
        let request = ExportRequest {
            project: root.join("fixtures/sample"),
            preset,
            output: artifacts.join(platform),
            cache: presets.join("cache"),
            device: None,
            allow_emulator: false,
        };
        let mut events = Vec::new();
        let summary = ara_export::export(&request, &mut events).unwrap();
        use ara_export::event::EventSink;
        let mut stream = ara_export::event::Ndjson(
            std::fs::File::create(artifacts.join(format!("{platform}.ndjson"))).unwrap(),
        );
        for event in &events {
            stream.emit(event).unwrap();
        }
        println!(
            "{platform}: {} ms, artifacts {:?}, verification {}",
            summary.elapsed_ms, summary.artifacts, summary.verify.method
        );
        assert!(summary.verify.passed);
        assert!(request.output.join("report.json").is_file());
        if platform == "windows" {
            assert!(summary.verify.runtime_executed);
        }
        if platform == "android" {
            ara_export::android::validate_package(&request.output.join("game.apk"), false).unwrap();
            ara_export::android::validate_package(&request.output.join("game.aab"), true).unwrap();
        }
    }
}
