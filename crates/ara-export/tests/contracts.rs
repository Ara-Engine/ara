#![allow(clippy::unwrap_used)]
use ara_export::{ExportPreset, Platform, android, doctor, template, tools};
use serde_json::json;
use std::collections::BTreeMap;

fn preset(platform: &str) -> ExportPreset {
    let arches = if platform == "android" {
        vec!["arm64-v8a", "x86_64"]
    } else {
        vec!["x64"]
    };
    ExportPreset::from_text(&json!({"$schema":"ara://schema/export/1","platform":platform,"scriptProject":"game/Game.csproj","assembly":"Game","content":"content","templates":arches.into_iter().map(|arch|json!({"platform":platform,"arch":arch,"config":"release","hash":"0".repeat(64),"source":"templates/runtime"})).collect::<Vec<_>>()} ).to_string()).unwrap()
}
#[test]
fn preset_is_schema_registered_and_canonical_round_trip_preserves_defaults() {
    let p = preset("android");
    let text = p.to_text().unwrap();
    assert!(!text.contains("chunkSize"));
    assert!(!text.contains("targetApi"));
    assert_eq!(
        text,
        ExportPreset::from_text(&text).unwrap().to_text().unwrap()
    );
    let mut db = ara_schema::SchemaDb::new();
    ExportPreset::register(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(db.get("ExportPreset").is_some());
    assert!(db.get("AndroidSigning").is_some());
    assert!(ExportPreset::from_text(&text.replace("\"android\"", "\"Bad\"")).is_err());
    let mut bad = p.clone();
    bad.target_api = 35;
    assert!(bad.validate().is_err());
    bad = p;
    bad.templates.pop();
    assert!(bad.validate().is_err());
    assert!(ExportPreset::from_text("{\"$schema\":\"ara://schema/export/1\",\"platform\":\"Windows\",\"platform\":\"Linux\"}").is_err());
}
#[test]
fn permission_merge_deduplicates_and_rejects_conflicts_and_xml_injection() {
    let p = preset("android");
    let a=r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"><uses-permission android:name="android.permission.CAMERA"/></manifest>"#.to_string();
    let b=r#"<manifest xmlns:a="http://schemas.android.com/apk/res/android"><uses-permission a:name="android.permission.RECORD_AUDIO"/></manifest>"#.to_string();
    let first = android::merge_manifest(&p, &[a.clone(), b.clone(), a.clone()]).unwrap();
    let second = android::merge_manifest(&p, &[b, a.clone()]).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.matches("android.permission.CAMERA").count(), 1);
    let conflict = a.replace("/>", " android:maxSdkVersion=\"30\"/>");
    assert!(android::merge_manifest(&p, &[a, conflict]).is_err());
    assert!(android::merge_manifest(&p,&["<!DOCTYPE x [<!ENTITY secret SYSTEM 'file:///secret'>]><manifest>&secret;</manifest>".into()]).is_err());
}
#[test]
fn template_cache_is_content_addressed_and_rejects_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("runtime");
    std::fs::write(&source, b"runtime").unwrap();
    let mut p = preset("windows");
    p.templates[0].source = source.to_string_lossy().into();
    p.templates[0].hash = blake3::hash(b"runtime").to_hex().to_string();
    let cache = dir.path().join("cache");
    let cached = template::resolve(&p.templates[0], dir.path(), &cache).unwrap();
    std::fs::remove_file(source).unwrap();
    assert_eq!(
        cached,
        template::resolve(&p.templates[0], dir.path(), &cache).unwrap()
    );
    std::fs::write(cached, b"poison").unwrap();
    assert!(template::resolve(&p.templates[0], dir.path(), &cache).is_err());
}
#[test]
fn deterministic_zip_and_elf_reject_wrong_architecture_and_alignment() {
    let dir = tempfile::tempdir().unwrap();
    let entries = BTreeMap::from([("b".into(), b"2".to_vec()), ("a".into(), b"1".to_vec())]);
    let a = dir.path().join("a.zip");
    let b = dir.path().join("b.zip");
    android::deterministic_zip(&entries, &a).unwrap();
    android::deterministic_zip(&entries, &b).unwrap();
    assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
    let mut elf = vec![0; 120];
    elf[..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2;
    elf[5] = 1;
    elf[18..20].copy_from_slice(&183u16.to_le_bytes());
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());
    elf[64..68].copy_from_slice(&1u32.to_le_bytes());
    elf[112..120].copy_from_slice(&16384u64.to_le_bytes());
    android::validate_elf(&elf, 183, 16384).unwrap();
    assert!(android::validate_elf(&elf, 62, 16384).is_err());
    elf[112..120].copy_from_slice(&4096u64.to_le_bytes());
    assert!(android::validate_elf(&elf, 183, 16384).is_err());
}
#[test]
fn doctor_versions_and_platform_scoping_have_stable_fixable_diagnostics() {
    assert!(doctor::version_matches("10.0.202", 10, 0));
    assert!(!doctor::version_matches("9.0.1", 10, 0));
    assert_eq!(
        tools::version("openjdk version \"17.0.12\""),
        vec![17, 0, 12]
    );
    let dir = tempfile::tempdir().unwrap();
    let t = tools::Toolchain {
        sdk: None,
        ndk: None,
        build_tools: None,
        java_home: None,
        zig: None,
        zigcc: None,
        bundletool: None,
        vswhere: None,
    };
    let opts = doctor::DoctorOptions {
        platform: Some(Platform::Android),
        target_api: 36,
        devices: false,
        scratch: dir.path().into(),
    };
    let checks = doctor::doctor(&opts, &t);
    assert!(
        !checks
            .iter()
            .any(|c| c.code.starts_with("ARA-EXP-LNX") || c.code.starts_with("ARA-EXP-WIN"))
    );
    for code in ["ARA-EXP-AND-001", "ARA-EXP-AND-002", "ARA-EXP-AND-006"] {
        let c = checks.iter().find(|c| c.code == code).unwrap();
        assert_eq!(c.status, doctor::Status::Error);
        assert!(!c.fixes.is_empty());
    }
}
#[test]
fn failed_exports_emit_coded_errors_and_keep_failure_report() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = preset("windows");
    p.templates[0].source = "missing-runtime".into();
    let request = ara_export::ExportRequest {
        project: dir.path().into(),
        preset: p,
        output: dir.path().join("dist"),
        cache: dir.path().join("cache"),
        device: None,
        allow_emulator: false,
    };
    let mut events = Vec::new();
    assert!(ara_export::export(&request, &mut events).is_err());
    assert!(matches!(
        events.last().unwrap().kind,
        ara_export::event::EventKind::Error(_)
    ));
    let job = std::fs::read_dir(request.cache.join("jobs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(job.join("report.json").is_file());
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, i as u64 + 1);
    }
}
