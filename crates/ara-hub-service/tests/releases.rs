#![allow(clippy::unwrap_used)]
use ara_hub_service::{DirectoryTemplates, EngineBuild, Hub, HubConfig, UpdateConfig};
use ara_proto::Endpoint;
use ara_update::{
    Channel, Kind,
    testing::{self, Response, Server},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn hub(root: &Path, server: &Server) -> Hub {
    Hub::open_with_updates(
        HubConfig {
            directory: root.join("config"),
            engine: EngineBuild {
                version: "0.0.1-alpha".into(),
                executable: std::env::current_exe().unwrap(),
                editor: None,
            },
            templates: Box::new(DirectoryTemplates(root.join("templates"))),
        },
        UpdateConfig {
            feed: server.config(),
            engines: root.join("engines"),
        },
    )
    .unwrap()
}
fn available(hub: &mut Hub) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let value = hub
            .call("hub.engines.available", json!({"channel":"preview"}))
            .unwrap();
        if value["checking"] == false {
            return value;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn finished(hub: &mut Hub) -> Value {
    finished_job(hub, None)
}
fn finished_job(hub: &mut Hub, id: Option<&str>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let value = hub.call("hub.downloads", json!({})).unwrap();
        let job = value["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| id.is_none_or(|id| job["id"] == id))
            .unwrap();
        if job["state"] != "running" {
            return job.clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn engine_download_verify_extract_register_and_managed_uninstall() {
    let root = testing::scratch("service-install");
    let server = Server::new();
    let release = server.release(
        Kind::Engine,
        "1.0.0",
        Channel::Preview,
        testing::engine_zip("1.0.0"),
    );
    server.releases(vec![release]);
    let mut hub = hub(&root, &server);
    assert_eq!(available(&mut hub)["releases"][0]["version"], "1.0.0");
    let started = Instant::now();
    let started_job = hub
        .call("hub.engine.install", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(started_job["phase"], "downloading");
    assert_eq!(started_job["cancellable"], true);
    assert!(
        started_job["disk"]["availableBytes"].as_u64().unwrap()
            >= started_job["disk"]["requiredBytes"].as_u64().unwrap()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    let job = finished(&mut hub);
    assert_eq!(job["state"], "complete", "{job}");
    let builds = hub.call("hub.engines", json!({})).unwrap();
    assert!(
        builds["engines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["version"] == "1.0.0" && e["managed"] == true)
    );
    assert!(root.join("engines/1.0.0/ara-editor.exe").is_file());
    assert!(!root.join("engines/.install-1.0.0").exists());
    let removal = hub
        .call("hub.engine.uninstall", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(removal["kind"], "uninstall");
    assert_eq!(removal["phase"], "preparing");
    assert_eq!(
        finished_job(&mut hub, removal["id"].as_str())["state"],
        "complete"
    );
    assert!(!root.join("engines/1.0.0").exists());
    assert!(
        !hub.call("hub.engines", json!({})).unwrap()["engines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["version"] == "1.0.0")
    );
}
#[test]
fn repair_restores_a_missing_companion_from_the_same_signed_version() {
    let root = testing::scratch("engine-repair");
    let server = Server::new();
    server.releases(vec![server.release(
        Kind::Engine,
        "1.0.0",
        Channel::Stable,
        testing::engine_zip("1.0.0"),
    )]);
    let mut hub = hub(&root, &server);
    available(&mut hub);
    let plan = hub
        .call("hub.engine.preflight", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(plan["destination"], json!(root.join("engines/1.0.0")));
    assert_eq!(plan["spaceBasis"], "estimate");
    assert!(
        !root.join("engines/1.0.0").exists(),
        "preflight is read-only"
    );
    hub.call("hub.engine.install", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(finished(&mut hub)["state"], "complete");
    fs::remove_file(root.join("engines/1.0.0/ara-player.exe")).unwrap();
    let job = hub
        .call("hub.engine.repair", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(job["kind"], "repair");
    assert_eq!(
        finished_job(&mut hub, job["id"].as_str())["state"],
        "complete"
    );
    assert!(root.join("engines/1.0.0/ara-player.exe").is_file());
    assert!(!root.join("engines/.repair-backup-1.0.0").exists());
}

#[test]
fn corrupted_repair_preserves_the_installed_engine() {
    let root = testing::scratch("engine-repair-tampered");
    let server = Server::new();
    let bytes = testing::engine_zip("1.0.0");
    server.releases(vec![server.release(
        Kind::Engine,
        "1.0.0",
        Channel::Stable,
        bytes.clone(),
    )]);
    let mut hub = hub(&root, &server);
    available(&mut hub);
    hub.call("hub.engine.install", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(finished(&mut hub)["state"], "complete");
    let original = fs::read(root.join("engines/1.0.0/ara.exe")).unwrap();
    server.set(
        "/assets/engine-v1.0.0/ara-engine-1.0.0-windows-x64.zip",
        Response::ok(vec![1; bytes.len()]),
    );
    let job = hub
        .call("hub.engine.repair", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(
        finished_job(&mut hub, job["id"].as_str())["state"],
        "failed"
    );
    assert_eq!(
        fs::read(root.join("engines/1.0.0/ara.exe")).unwrap(),
        original
    );
    assert!(root.join("engines/1.0.0/ara-editor.exe").is_file());
}

#[test]
fn unfinished_managed_uninstall_can_retry_cleanup_without_an_engine_registration() {
    let root = testing::scratch("uninstall-cleanup");
    let server = Server::new();
    let mut hub = hub(&root, &server);
    let trash = root.join("engines/.uninstall-1.0.0");
    fs::create_dir_all(&trash).unwrap();
    fs::write(trash.join(".ara-managed.json"), br#"{"version":"1.0.0"}"#).unwrap();
    fs::write(
        trash.join("locked-before-retry.txt"),
        b"left by an interrupted cleanup",
    )
    .unwrap();
    let job = hub
        .call("hub.engine.uninstall", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(
        job["cancellable"], false,
        "registration was committed before cleanup retry"
    );
    assert_eq!(
        finished_job(&mut hub, job["id"].as_str())["state"],
        "complete"
    );
    assert!(!trash.exists());
}

#[test]
fn tampered_and_oversized_assets_never_install_or_register() {
    for (name, body, expected) in [
        ("tamper", vec![1; 2048], ara_update::HASH),
        ("oversize", vec![0; 4096], ara_update::SIZE),
    ] {
        let root = testing::scratch(name);
        let server = Server::new();
        let original = vec![0; 2048];
        let release = server.release(Kind::Engine, "1.0.0", Channel::Stable, original);
        server.releases(vec![release]);
        server.set(
            "/assets/engine-v1.0.0/ara-engine-1.0.0-windows-x64.zip",
            Response::ok(body),
        );
        let mut hub = hub(&root, &server);
        available(&mut hub);
        hub.call("hub.engine.install", json!({"version":"1.0.0"}))
            .unwrap();
        let job = finished(&mut hub);
        assert_eq!(job["state"], "failed");
        assert_eq!(
            serde_json::from_value::<ara_diag::Diagnostic>(job["error"].clone())
                .unwrap()
                .code,
            expected
        );
        assert!(!root.join("engines/1.0.0").exists());
        assert!(!root.join("engines/.install-1.0.0").exists());
    }
}
#[test]
fn bad_signature_and_wrong_key_are_catalogued_and_no_install_is_started() {
    let mut wrong_seed = testing::test_seed();
    let replacement = if wrong_seed.starts_with('0') {
        "1"
    } else {
        "0"
    };
    wrong_seed.replace_range(..1, replacement);
    for signature in [
        "bad".into(),
        ara_update::sign(b"other", &wrong_seed).unwrap().0,
    ] {
        let root = testing::scratch("signature");
        let server = Server::new();
        let release = server.release(
            Kind::Engine,
            "1.0.0",
            Channel::Stable,
            testing::engine_zip("1.0.0"),
        );
        server.releases(vec![release]);
        server.set(
            "/assets/engine-v1.0.0/release.json.sig",
            Response::ok(signature),
        );
        let mut hub = hub(&root, &server);
        let result = available(&mut hub);
        assert_eq!(result["status"]["state"], "error");
        let diagnostic: ara_diag::Diagnostic =
            serde_json::from_value(result["status"]["diagnostic"].clone()).unwrap();
        assert_eq!(diagnostic.code, ara_update::SIGNATURE);
        assert!(
            hub.call("hub.engine.install", json!({"version":"1.0.0"}))
                .is_err()
        );
        assert!(!root.join("engines/1.0.0").exists());
    }
}
#[test]
fn register_missing_project_pin_and_refuse_external_uninstall() {
    let root = testing::scratch("missing-pin");
    let server = Server::new();
    let mut hub = hub(&root, &server);
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(
        project.join("ara.project.json"),
        r#"{"name":"Missing","layoutVersion":4,"engineVersion":"1.0.0"}"#,
    )
    .unwrap();
    assert_eq!(
        hub.call("hub.register", json!({"path":project})).unwrap()["engineVersion"],
        "1.0.0"
    );
    assert!(
        hub.call("hub.engine.uninstall", json!({"version":"0.0.1-alpha"}))
            .is_err()
    );
}

#[test]
fn engine_minimum_hub_version_cannot_block_the_hub_update_it_requires() {
    let server = Server::new();
    let root = testing::scratch("minimum-hub");
    let listings: Vec<_> = [Kind::Hub, Kind::Engine]
        .into_iter()
        .map(|kind| server.release(kind, "1.0.0", Channel::Stable, vec![1; 32]))
        .collect();
    server.releases(listings);
    let feed = ara_update::Feed::new(server.config(), &root.join("initial-cache")).unwrap();
    for release in feed.check().unwrap().releases {
        let mut manifest = release.manifest().clone();
        manifest.min_hub_version = "9.0.0".into();
        let bytes = ara_update::canonical_manifest(&manifest).unwrap();
        let (signature, _) = ara_update::sign(&bytes, &testing::test_seed()).unwrap();
        let prefix = format!("/assets/{}-v1.0.0", manifest.kind.name());
        server.set(&format!("{prefix}/release.json"), Response::ok(bytes));
        server.set(
            &format!("{prefix}/release.json.sig"),
            Response::ok(signature),
        );
    }
    let mut hub = hub(&root, &server);
    available(&mut hub);
    assert!(
        hub.call("hub.engine.install", json!({"version":"1.0.0"}))
            .is_err()
    );
    hub.call("hub.update.prepare", json!({"version":"1.0.0"}))
        .unwrap();
    assert_eq!(finished(&mut hub)["state"], "complete");
}

#[test]
fn an_install_never_replaces_a_build_registered_while_the_download_was_running() {
    let server = Server::new();
    let root = testing::scratch("register-race");
    let bytes = testing::engine_zip("1.0.0");
    let release = server.release(Kind::Engine, "1.0.0", Channel::Stable, bytes.clone());
    server.releases(vec![release]);
    server.set(
        "/assets/engine-v1.0.0/ara-engine-1.0.0-windows-x64.zip",
        Response {
            delay_ms: 300,
            ..Response::ok(bytes)
        },
    );
    let mut hub = hub(&root, &server);
    available(&mut hub);
    hub.call("hub.engine.install", json!({"version":"1.0.0"}))
        .unwrap();
    let executable = std::env::current_exe().unwrap();
    hub.call(
        "hub.engine.register",
        json!({"engine":{"version":"1.0.0","executable":executable,"editor":null}}),
    )
    .unwrap();
    assert_eq!(finished(&mut hub)["state"], "failed");
    assert!(!root.join("engines/1.0.0").exists());
    let engines = hub.call("hub.engines", json!({})).unwrap();
    let preserved = engines["engines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["version"] == "1.0.0")
        .unwrap();
    assert_eq!(preserved["executable"], json!(executable));
}
