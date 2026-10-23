//! Real toolchain evidence. Prepare conformance pins and the Fox demo content first.
#![allow(clippy::unwrap_used)]
use ara_export::{ExportPreset, ExportRequest, preset::PackageFormat};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn wait(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("verification child timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn assert_packages_exclude_private_artifacts(path: &Path) {
    let entries = ara_export::android::zip_entries(path).unwrap();
    assert!(entries.contains_key("ara-player.exe"));
    assert!(entries.keys().all(|p| !p.contains("symbols/")
        && !p.ends_with(".pdb")
        && !p.ends_with(".log")
        && !p.ends_with(".dmp")));
}

#[test]
#[ignore = "requires real pins, SDKs and staged Fox demo; see README Distribution validation"]
fn packages_fox_content_and_symbolizes_exported_player_crashes() {
    let root = PathBuf::from(std::env::var_os("ARA_DISTRIBUTION_EVIDENCE").expect("evidence root"));
    let project = root.join("project");
    let mut reports = Vec::new();
    let mut windows = None;
    for (name, base, format) in [
        (
            "windows-portable",
            "windows",
            PackageFormat::WindowsPortable,
        ),
        (
            "windows-installer",
            "windows",
            PackageFormat::WindowsInstaller,
        ),
        ("linux-tar", "linux", PackageFormat::LinuxTar),
        ("linux-appimage", "linux", PackageFormat::LinuxAppimage),
        ("android-play", "android", PackageFormat::AndroidPlay),
    ] {
        let mut preset = ExportPreset::from_text(
            &std::fs::read_to_string(root.join("presets").join(format!("{base}.export.json")))
                .unwrap(),
        )
        .unwrap();
        preset.packaging = format;
        let request = ExportRequest {
            project: project.clone(),
            preset,
            output: root.join(name),
            cache: root.join("cache"),
            device: None,
            allow_emulator: false,
        };
        let mut events = Vec::new();
        let resumed = std::env::var("ARA_DISTRIBUTION_RESUME").as_deref() == Ok("1")
            && request.output.join("report.json").is_file();
        let summary: ara_export::ExportSummary = if resumed {
            serde_json::from_slice(&std::fs::read(request.output.join("report.json")).unwrap())
                .unwrap()
        } else {
            ara_export::export(&request, &mut events).unwrap()
        };
        use ara_export::event::EventSink;
        if !resumed {
            let mut stream = ara_export::event::Ndjson(
                std::fs::File::create(root.join(format!("{name}.ndjson"))).unwrap(),
            );
            for event in events {
                stream.emit(&event).unwrap();
            }
        }
        assert!(summary.verify.passed);
        assert!(request.output.join("build/game.arapack").is_file());
        let manifest: ara_export::distribution::DistributionManifest = serde_json::from_slice(
            &std::fs::read(request.output.join("distribution.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(summary.build_id, manifest.build_id);
        for (name, identity) in &manifest.files {
            ara_pack::validate_path(name).unwrap();
            let bytes = std::fs::read(request.output.join("build").join(name)).unwrap();
            assert_eq!(blake3::hash(&bytes).to_hex().as_str(), identity.hash);
            if resumed && format == PackageFormat::WindowsInstaller {
                let installed = std::fs::read(root.join("installed-game").join(name)).unwrap();
                assert_eq!(bytes, installed);
            }
        }
        for (name, identity) in &manifest.packages {
            let bytes = std::fs::read(request.output.join(name)).unwrap();
            assert_eq!(blake3::hash(&bytes).to_hex().as_str(), identity.hash);
        }
        assert!(
            manifest
                .files
                .keys()
                .all(|p| !p.ends_with(".pdb") && !p.ends_with(".log"))
        );
        if base == "windows" {
            assert!(summary.verify.runtime_executed);
            assert!(
                summary.verify.evidence["texture"]
                    .as_str()
                    .unwrap()
                    .contains("packed demo files")
            );
            assert_eq!(
                summary.verify.evidence["template_contract"],
                "ara.player.conformance/1"
            );
            assert_packages_exclude_private_artifacts(&request.output.join("game.zip"));
            windows = Some((request.output.clone(), summary.build_id.clone()));
        }
        if format == PackageFormat::WindowsInstaller && !resumed {
            let installation = root.join("installed-game");
            let mut installer = Command::new(request.output.join("game-setup.exe"))
                .arg("--destination")
                .arg(&installation)
                .spawn()
                .unwrap();
            assert!(wait(&mut installer).success());
            assert!(installation.join("ara-player.exe").is_file());
            assert!(!installation.join("symbols").exists());
        }
        println!(
            "{name}: {} ms; packages {:?}",
            summary.elapsed_ms, manifest.packages
        );
        reports.push(serde_json::to_value(summary).unwrap());
    }
    let (windows, build_id) = windows.unwrap();
    let mut crashes = Vec::new();
    std::fs::create_dir_all(root.join("crashes")).unwrap();
    for mode in ["native", "panic", "managed"] {
        let directory = root.join("crashes").join(mode);
        std::fs::create_dir(&directory).unwrap();
        let log = std::fs::File::create(directory.join("process.log")).unwrap();
        let mut child = Command::new(windows.join("build/ara-player.exe"))
            .args(["--headless", "--crash-probe", mode])
            .env("ARA_CRASH_DIR", &directory)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        assert!(!wait(&mut child).success(), "{mode} probe must terminate");
        let extension = if mode == "native" {
            "dmp"
        } else if mode == "managed" {
            "managed"
        } else {
            "json"
        };
        let dump = std::fs::read_dir(&directory)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == extension))
            .expect("captured crash artifact");
        let started = Instant::now();
        let report = ara_crash::symbols::symbolize(
            &dump,
            &windows.join("symbols").join(&build_id).join("store"),
        )
        .unwrap();
        assert_eq!(report.build_id, build_id);
        assert!(!report.stack.is_empty());
        if mode == "native" {
            assert!(
                report.stack.contains("ara_crash") || report.stack.contains("ara_export"),
                "{}",
                report.stack
            );
        }
        if mode == "panic" {
            assert!(
                report.stack.contains("ara_export_player::runtime::run"),
                "{}",
                report.stack
            );
        }
        if mode == "managed" {
            assert!(report.stack.contains("ThrowProbe"), "{}", report.stack);
        }
        ara_export::tools::write_json(&directory.join("symbolized.json"), &report).unwrap();
        let native_dump = std::fs::read_dir(&directory)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "dmp"));
        crashes.push(serde_json::json!({"kind":mode,"recordBytes":std::fs::metadata(dump).unwrap().len(),"dumpBytes":native_dump.map(|p| std::fs::metadata(p).unwrap().len()),"symbolizeMs":started.elapsed().as_millis(),"diagnostic":report.diagnostic()}));
    }
    ara_export::tools::write_json(&root.join("verification.json"), &serde_json::json!({"format":"ara.distribution.validation/1","project":"crates/ara-player/demo (packed content)","template":"ara.player.conformance/1","exports":reports,"crashes":crashes,"windowsSigning":"unsigned; certificate was not provided","linuxExecution":"static ELF and embedded SquashFS verification only","androidSigning":"disposable test upload key; no Play upload performed"})).unwrap();
}
