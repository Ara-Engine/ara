#![cfg(feature = "test-support")]
#![allow(clippy::unwrap_used)]
use ara_update::{
    testing::{self, Response, Server},
    *,
};
use std::{fs, io::Write};

#[test]
fn download_exposes_verification_after_received_bytes() {
    let server = Server::new();
    let root = testing::scratch("download-phases");
    let data = vec![0x42; 4096];
    let listing = server.release(Kind::Hub, "1.0.0", Channel::Stable, data);
    server.releases(vec![listing]);
    let feed = Feed::new(server.config(), &root.join("cache")).unwrap();
    let release = feed.check().unwrap().releases.remove(0);
    let mut phases = Vec::new();
    let mut received = 0;
    download_with_phase(
        &feed,
        &release,
        &root.join("download"),
        &Cancel::default(),
        |progress| received = progress.received,
        |phase| phases.push(phase),
    )
    .unwrap();
    assert_eq!(received, 4096);
    assert!(matches!(
        phases.as_slice(),
        [DownloadPhase::Downloading, DownloadPhase::Verifying]
    ));
}

#[test]
fn feed_pagination_channels_etags_offline_and_rate_limit() {
    let server = Server::new();
    let root = testing::scratch("feed");
    let hub = server.release(Kind::Hub, "0.2.0", Channel::Stable, vec![1; 128]);
    let engine = server.release(Kind::Engine, "0.3.0-alpha", Channel::Preview, vec![2; 128]);
    server.set(
        "/repos/Ara-Engine/ara/releases?per_page=100",
        Response {
            headers: vec![
                ("ETag".into(), "\"one\"".into()),
                (
                    "Link".into(),
                    format!(
                        "<{}/repos/Ara-Engine/ara/releases?per_page=100&page=2>; rel=\"next\"",
                        server.base
                    ),
                ),
            ],
            ..Response::ok(serde_json::to_vec(&vec![hub]).unwrap())
        },
    );
    server.set(
        "/repos/Ara-Engine/ara/releases?per_page=100&page=2",
        Response {
            headers: vec![("ETag".into(), "\"two\"".into())],
            ..Response::ok(serde_json::to_vec(&vec![engine]).unwrap())
        },
    );
    let feed = Feed::new(server.config(), &root).unwrap();
    let snapshot = feed.check().unwrap();
    assert_eq!(snapshot.releases.len(), 2);
    assert_eq!(snapshot.status, FeedStatus::Fresh);
    assert_eq!(snapshot.releases[0].manifest().channel, Channel::Preview);
    let before = server.requests().len();
    assert_eq!(feed.check().unwrap().releases.len(), 2);
    let requests = server.requests();
    assert_eq!(requests.len() - before, 2);
    assert!(requests[before].headers.contains_key("if-none-match"));
    server.set(
        "/repos/Ara-Engine/ara/releases?per_page=100",
        Response {
            status: 429,
            headers: vec![("Retry-After".into(), "30".into())],
            ..Response::default()
        },
    );
    assert!(matches!(
        feed.check().unwrap().status,
        FeedStatus::RateLimited { .. }
    ));
    drop(server);
    assert!(matches!(
        feed.check().unwrap().status,
        FeedStatus::Offline { .. }
    ));
}
#[test]
fn no_cache_reports_rate_limit_and_wrong_keys_cannot_use_a_cache() {
    let server = Server::new();
    let root = testing::scratch("no-cache");
    server.set(
        "/repos/Ara-Engine/ara/releases?per_page=100",
        Response {
            status: 403,
            ..Response::default()
        },
    );
    assert_eq!(
        Feed::new(server.config(), &root)
            .unwrap()
            .check()
            .unwrap_err()
            .code,
        RATE_LIMIT
    );
    let release = server.release(Kind::Hub, "1.0.0", Channel::Stable, vec![4; 20]);
    server.releases(vec![release]);
    Feed::new(server.config(), &root).unwrap().check().unwrap();
    let mut config = FeedConfig::for_test(server.base.clone(), vec!["00".repeat(32)]);
    config.channel = Channel::Preview;
    assert_eq!(
        Feed::new(config, &root).unwrap().check().unwrap_err().code,
        SIGNATURE
    );
}
#[test]
fn signature_hash_size_resume_cancel_and_cleanup() {
    let server = Server::new();
    let root = testing::scratch("downloads");
    let data = vec![0x42; 200_000];
    let listing = server.release(Kind::Engine, "1.0.0", Channel::Stable, data.clone());
    server.releases(vec![listing]);
    let feed = Feed::new(server.config(), &root.join("cache")).unwrap();
    let release = feed.check().unwrap().releases.remove(0);
    let path = "/assets/engine-v1.0.0/ara-engine-1.0.0-windows-x64.zip";
    server.set(
        path,
        Response {
            interrupt: Some(100_000),
            ..Response::ok(data.clone())
        },
    );
    let stage = root.join("resume");
    assert_eq!(
        download(&feed, &release, &stage, &Cancel::default(), |_| {})
            .unwrap_err()
            .code,
        NETWORK
    );
    let complete = download(&feed, &release, &stage, &Cancel::default(), |_| {}).unwrap();
    assert_eq!(fs::read(complete).unwrap(), data);
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.headers.get("range").is_some_and(|s| s == "bytes=100000-"))
    );
    server.set(path, Response::ok(vec![0x43; 200_000]));
    let stage = root.join("tamper");
    assert_eq!(
        download(&feed, &release, &stage, &Cancel::default(), |_| {})
            .unwrap_err()
            .code,
        HASH
    );
    assert_eq!(fs::read_dir(stage).unwrap().count(), 0);
    server.set(path, Response::ok(vec![0x42; 200_001]));
    let stage = root.join("oversize");
    assert_eq!(
        download(&feed, &release, &stage, &Cancel::default(), |_| {})
            .unwrap_err()
            .code,
        SIZE
    );
    assert_eq!(fs::read_dir(stage).unwrap().count(), 0);
    server.set(path, Response::ok(vec![0x42; 200_000]));
    let stage = root.join("cancel");
    let cancel = Cancel::default();
    assert_eq!(
        download(&feed, &release, &stage, &cancel, |_| cancel.cancel())
            .unwrap_err()
            .code,
        CANCELLED
    );
    assert_eq!(fs::read_dir(stage).unwrap().count(), 0);
    let (bytes, sig) = release.proof();
    assert_eq!(
        verify(bytes.as_bytes(), b"bad", &[testing::test_public_key()])
            .unwrap_err()
            .code,
        SIGNATURE
    );
    assert_eq!(
        verify(bytes.as_bytes(), sig.as_bytes(), &["00".repeat(32)])
            .unwrap_err()
            .code,
        SIGNATURE
    );
}
#[test]
fn archive_traversal_rejects_without_partial_tree() {
    let root = testing::scratch("zip");
    let archive = root.join("unsafe.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    writer
        .start_file("../outside", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"bad").unwrap();
    writer.finish().unwrap();
    let stage = root.join("stage");
    assert_eq!(
        extract(&archive, &stage, &Cancel::default())
            .unwrap_err()
            .code,
        MANIFEST
    );
    assert!(!stage.exists());
    assert!(!root.join("outside").exists());
}

#[test]
fn cancel_interrupts_a_stalled_body_and_redirects_cannot_escape_the_allowed_origin() {
    let server = Server::new();
    let root = testing::scratch("stalled-cancel");
    let listing = server.release(Kind::Engine, "1.0.0", Channel::Stable, vec![3; 200_000]);
    server.releases(vec![listing]);
    let feed = Feed::new(server.config(), &root.join("cache")).unwrap();
    let release = feed.check().unwrap().releases.remove(0);
    let asset = "/assets/engine-v1.0.0/ara-engine-1.0.0-windows-x64.zip";
    server.set(
        asset,
        Response {
            delay_ms: 2_000,
            ..Response::ok(vec![3; 200_000])
        },
    );
    let cancel = Cancel::default();
    let flag = cancel.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        flag.cancel();
    });
    let now = std::time::Instant::now();
    let stage = root.join("stage");
    assert_eq!(
        download(&feed, &release, &stage, &cancel, |_| {})
            .unwrap_err()
            .code,
        CANCELLED
    );
    assert!(
        now.elapsed() < std::time::Duration::from_secs(1),
        "cancellation waited for the stalled response"
    );
    worker.join().unwrap();
    assert_eq!(fs::read_dir(stage).unwrap().count(), 0);
    // Allow the fake server's intentionally delayed response to finish before the next request.
    drop(server);
    let server = Server::new();
    let listing = server.release(Kind::Engine, "1.0.0", Channel::Stable, vec![3; 200_000]);
    server.releases(vec![listing]);
    let feed = Feed::new(server.config(), &root.join("redirect-cache")).unwrap();
    let release = feed.check().unwrap().releases.remove(0);
    let other = Server::new();
    server.set(
        asset,
        Response {
            status: 302,
            headers: vec![("Location".into(), format!("{}/forbidden", other.base))],
            ..Response::default()
        },
    );
    assert_eq!(
        download(
            &feed,
            &release,
            &root.join("redirect"),
            &Cancel::default(),
            |_| {}
        )
        .unwrap_err()
        .code,
        URL
    );
    assert!(other.requests().is_empty());
}
#[test]
#[cfg(windows)]
fn a_locked_hub_reports_the_failed_swap_and_preserves_both_builds() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = testing::scratch("locked-swap");
    let current = root.join("Ara Hub");
    let staged = root.join("new");
    fs::create_dir(&current).unwrap();
    fs::create_dir(&staged).unwrap();
    let locked = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(current.join("ara-hub.exe"))
        .unwrap();
    fs::write(staged.join("ara-hub.exe"), b"new").unwrap();
    let error = updater::swap(&current, &staged, |_| {
        panic!("locked build must not launch")
    })
    .unwrap_err();
    assert_eq!(error.code, IO);
    assert!(
        error.message.contains("move current Hub to backup"),
        "{error}"
    );
    assert!(
        error.message.contains(&current.display().to_string()),
        "{error}"
    );
    assert!(
        error
            .message
            .contains(&current.with_extension("old").display().to_string()),
        "{error}"
    );
    assert!(current.join("ara-hub.exe").is_file());
    assert!(staged.join("ara-hub.exe").is_file());
    assert!(!current.with_extension("old").exists());
    drop(locked);
    updater::swap(&current, &staged, |_| Ok(())).unwrap();
}

#[test]
fn atomic_hub_swap_and_startup_rollback_with_dummy_executables() {
    let root = testing::scratch("swap");
    let current = root.join("Ara Hub");
    let stage = root.join("new");
    fs::create_dir(&current).unwrap();
    fs::create_dir(&stage).unwrap();
    fs::write(current.join("ara-hub.exe"), b"old").unwrap();
    fs::write(stage.join("ara-hub.exe"), b"new").unwrap();
    assert_eq!(
        updater::swap(&current, &stage, |path| {
            assert_eq!(fs::read(path.join("ara-hub.exe")).unwrap(), b"new");
            Err(Error {
                code: IO,
                message: "dummy startup failed".into(),
            })
        })
        .unwrap_err()
        .code,
        ROLLBACK
    );
    assert_eq!(fs::read(current.join("ara-hub.exe")).unwrap(), b"old");
    updater::swap(&current, &stage, |_| Ok(())).unwrap();
    assert_eq!(fs::read(current.join("ara-hub.exe")).unwrap(), b"new");
    assert!(!current.with_extension("old").exists());
}

#[test]
#[cfg(windows)]
fn updater_verifies_and_health_checks_dummy_processes_without_a_window() {
    let root = testing::scratch("process-update");
    // Keep successful dummy Hubs from surviving this test, including assertion failures.
    struct Processes(std::path::PathBuf);
    impl Drop for Processes {
        fn drop(&mut self) {
            for state in ["healthy", "failed"] {
                if let Ok(pids) = fs::read_to_string(self.0.join(state).join("Ara Hub/pids.txt")) {
                    for pid in pids.lines().filter_map(|pid| pid.parse().ok()) {
                        if let Err(error) = updater::wait_for_parent(pid) {
                            eprintln!("dummy cleanup: {error}");
                        }
                    }
                }
            }
        }
    }
    let _processes = Processes(root.clone());
    let source = root.join("dummy.rs");
    fs::write(
        &source,
        r#"fn main() {
        use std::io::Write;
        let exe = std::env::current_exe().expect("exe");
        let mut pids = std::fs::OpenOptions::new().create(true).append(true)
            .open(exe.parent().expect("parent").join("pids.txt")).expect("pid file");
        writeln!(pids, "{}", std::process::id()).expect("pid");
        let args: Vec<_> = std::env::args().collect();
        let Some(index) = args.iter().position(|s| s == "--update-health") else { return; };
        if exe.parent().expect("parent").join("fail.txt").exists() { std::process::exit(9); }
        std::fs::write(&args[index+1], b"ready").expect("health");
        std::thread::sleep(std::time::Duration::from_secs(2));
    }"#,
    )
    .unwrap();
    let dummy = root.join("dummy.exe");
    let output = std::process::Command::new("rustc")
        .arg(&source)
        .arg("-o")
        .arg(&dummy)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(dummy).unwrap();
    for failed in [false, true] {
        let directory = root.join(if failed { "failed" } else { "healthy" });
        fs::create_dir(&directory).unwrap();
        let current = directory.join("Ara Hub");
        fs::create_dir(&current).unwrap();
        fs::write(current.join("ara-hub.exe"), &bytes).unwrap();
        let old_marker = br#"{"format":"ara.hub/1","version":"1.0.0","build":{"commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","generation":1}}"#;
        fs::write(current.join("hub.json"), old_marker).unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("ara-hub.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bytes).unwrap();
        for name in [
            "ara-hub-updater.exe",
            "bootstrap/ara.exe",
            "bootstrap/service.json",
            "THIRD_PARTY_NOTICES.txt",
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fixture").unwrap();
        }
        zip.start_file("hub.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"{"format":"ara.hub/1","version":"1.0.0","build":{"commit":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","generation":2}}"#)
            .unwrap();
        if failed {
            zip.start_file("fail.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fail startup").unwrap();
        }
        let server = Server::new();
        let listing = server.release_build(
            Kind::Hub,
            "1.0.0",
            Channel::Stable,
            zip.finish().unwrap().into_inner(),
            Some(BuildIdentity {
                commit: "b".repeat(40),
                generation: 2,
            }),
        );
        server.releases(vec![listing]);
        let feed = Feed::new(server.config(), &directory.join("cache")).unwrap();
        let release = feed.check().unwrap().releases.remove(0);
        let archive = download(
            &feed,
            &release,
            &directory.join("downloads"),
            &Cancel::default(),
            |_| {},
        )
        .unwrap();
        let config = directory.join("config");
        fs::create_dir(&config).unwrap();
        let (manifest, signature) = release.proof();
        let plan = updater::Plan {
            current: current.clone(),
            archive,
            manifest: manifest.into(),
            signature: signature.into(),
            config,
            ara: directory.join("external-service.exe"),
        };
        let result = updater::apply_for_test(&plan, &[testing::test_public_key()]);
        if failed {
            assert_eq!(result.unwrap_err().code, ROLLBACK);
            assert_eq!(fs::read(current.join("hub.json")).unwrap(), old_marker);
        } else {
            result.unwrap();
            let installed = fs::read(current.join("hub.json")).unwrap();
            updater::apply_for_test(&plan, &[testing::test_public_key()]).unwrap();
            assert_eq!(
                fs::read(current.join("hub.json")).unwrap(),
                installed,
                "same build must not be swapped again"
            );
            assert!(!current.with_extension("old").exists());
            assert!(
                fs::read_to_string(current.join("hub.json"))
                    .unwrap()
                    .contains("1.0.0")
            );
        }
    }
}
