//! Real keystore-config path using the disposable sample debug key as a test upload key.
#![allow(clippy::unwrap_used)]
use ara_export::{ExportPreset, preset::AndroidSigning};
use std::path::PathBuf;
#[test]
#[ignore = "requires prepared sample, Android toolchain, exported Android payload and test password environment"]
fn signs_apk_and_aab_using_configured_keystore_and_environment_passwords() {
    let presets = PathBuf::from(std::env::var_os("ARA_EXPORT_PRESETS").unwrap());
    let payload = PathBuf::from(std::env::var_os("ARA_EXPORT_ANDROID_PAYLOAD").unwrap());
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let work = dir.path().join("work");
    let dist = dir.path().join("dist");
    for p in [&project, &work, &dist] {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::copy(
        presets.join("cache/jobs/debug.keystore"),
        project.join("upload.jks"),
    )
    .unwrap();
    let mut preset = ExportPreset::from_text(
        &std::fs::read_to_string(presets.join("android.export.json")).unwrap(),
    )
    .unwrap();
    preset.signing = Some(AndroidSigning {
        keystore: "upload.jks".into(),
        alias: "androiddebugkey".into(),
        store_password_env: "ARA_TEST_STORE_PASSWORD".into(),
        key_password_env: "ARA_TEST_KEY_PASSWORD".into(),
    });
    let toolchain = ara_export::tools::Toolchain::discover();
    let checks = ara_export::doctor::doctor_for_preset(
        &project,
        &preset,
        &ara_export::doctor::DoctorOptions {
            platform: Some(ara_export::Platform::Android),
            target_api: 36,
            devices: false,
            scratch: work.join("doctor"),
        },
        &toolchain,
    );
    assert_eq!(
        checks
            .iter()
            .find(|c| c.code == "ARA-EXP-AND-007")
            .unwrap()
            .status,
        ara_export::doctor::Status::Ok
    );
    let packages =
        ara_export::android::package(&project, &preset, &toolchain, &payload, &work, &dist)
            .unwrap();
    assert!(packages.apk.is_file());
    assert!(packages.aab.is_file());
}
