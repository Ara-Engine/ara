//! Explicit same-version SDK replacement preserves external files and rollback authority.
use ara_hub_service::{DirectoryTemplates, EngineBuild, Hub, HubConfig, UpdateConfig};
use ara_proto::Endpoint;
use ara_update::{
    Channel, Kind,
    testing::{self, Response, Server},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const VERSION: &str = ara_host::project_layout::CURRENT_ENGINE_VERSION;

fn setup(root: &Path, server: &Server) -> (Hub, PathBuf) {
    let external = root.join("external-development");
    fs::create_dir_all(&external).expect("external fixture");
    let executable = external.join("ara.exe");
    fs::write(&executable, b"external preview-three build").expect("external executable");
    let mut hub = Hub::open_with_updates(
        HubConfig {
            directory: root.join("profile"),
            engine: EngineBuild {
                version: VERSION.into(),
                executable: std::env::current_exe().expect("test executable"),
                editor: None,
            },
            templates: Box::new(DirectoryTemplates(root.join("templates"))),
        },
        UpdateConfig {
            feed: server.config(),
            engines: root.join("engines"),
        },
    )
    .expect("isolated Hub");
    hub.call(
        "hub.engine.register",
        json!({"engine":{"version":VERSION,"executable":executable,"editor":null}}),
    )
    .expect("external registration");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if hub
            .call("hub.engines.available", json!({"channel":"preview"}))
            .expect("release feed")["checking"]
            == false
        {
            break;
        }
        assert!(Instant::now() < deadline, "feed check timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
    (hub, executable)
}

fn finished(hub: &mut Hub, id: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let jobs = hub.call("hub.downloads", json!({})).expect("jobs");
        let job = jobs["jobs"]
            .as_array()
            .expect("job list")
            .iter()
            .find(|job| job["id"] == *id)
            .expect("exact job");
        if job["state"] != "running" {
            return job.clone();
        }
        assert!(
            Instant::now() < deadline,
            "SDK replacement timed out: {job}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn registered(hub: &mut Hub) -> Value {
    hub.call("hub.engines", json!({})).expect("engines")["engines"]
        .as_array()
        .expect("engine list")
        .iter()
        .find(|engine| engine["version"] == VERSION)
        .expect("current registration")
        .clone()
}

#[test]
fn cancelled_tampered_and_incapable_signed_sdks_preserve_external_registration_and_files() {
    for failure in ["cancel", "tamper", "incapable"] {
        let root = testing::scratch(&format!("install-current-{failure}"));
        let server = Server::new();
        let archive = testing::engine_zip(VERSION);
        server.releases(vec![server.release(
            Kind::Engine,
            VERSION,
            Channel::Preview,
            archive.clone(),
        )]);
        let (mut hub, external) = setup(&root, &server);
        let original = fs::read(&external).expect("original external executable");
        let registration = registered(&mut hub);
        let route = format!("/assets/engine-v{VERSION}/ara-engine-{VERSION}-windows-x64.zip");
        if failure == "cancel" {
            server.set(
                &route,
                Response {
                    delay_ms: 300,
                    ..Response::ok(archive)
                },
            );
        } else if failure == "tamper" {
            server.set(&route, Response::ok(vec![0; archive.len()]));
        }
        let job = hub
            .call("hub.engine.installCurrent", json!({"version":VERSION}))
            .expect("explicit current installation");
        assert_eq!(job["kind"], "engine-current");
        if failure == "cancel" {
            hub.call("hub.download.cancel", json!({"job":job["id"]}))
                .expect("cancel before publication");
        }
        let done = finished(&mut hub, &job["id"]);
        assert_eq!(
            done["state"],
            if failure == "cancel" {
                "cancelled"
            } else {
                "failed"
            },
            "{done}"
        );
        assert_eq!(
            registered(&mut hub),
            registration,
            "{failure} keeps the exact registration"
        );
        assert_eq!(
            fs::read(&external).expect("external files remain"),
            original
        );
        assert!(!root.join("engines").join(VERSION).exists());
    }
}

fn capable_cli() -> Vec<u8> {
    #[cfg(windows)]
    {
        fs::read(
            std::env::var_os("ARA_TEST_ENGINE_CLI")
                .expect("set ARA_TEST_ENGINE_CLI to the built current ara.exe"),
        )
        .expect("current CLI fixture")
    }
    #[cfg(not(windows))]
    {
        format!("#!/bin/sh\nprintf '%s\\n' '{{\"engineVersion\":\"{VERSION}\",\"projectLayouts\":[4]}}'\n").into_bytes()
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires ARA_TEST_ENGINE_CLI pointing at the built current CLI"
)]
fn explicit_current_install_copies_external_then_replaces_managed_while_ordinary_repair_stays_strict()
 {
    let root = testing::scratch("install-current-signed-success");
    let server = Server::new();
    let archive = testing::engine_zip_with_cli(VERSION, &capable_cli(), Some("preview-four-build"));
    server.releases(vec![server.release(
        Kind::Engine,
        VERSION,
        Channel::Preview,
        archive,
    )]);
    let (mut hub, external) = setup(&root, &server);
    let original = fs::read(&external).expect("external before");
    let job = hub
        .call("hub.engine.installCurrent", json!({"version":VERSION}))
        .expect("explicit install current");
    assert_eq!(finished(&mut hub, &job["id"])["state"], "complete");
    let installed = root.join("engines").join(VERSION);
    assert_eq!(
        registered(&mut hub)["executable"],
        json!(installed.join("ara.exe"))
    );
    assert_eq!(fs::read(&external).expect("external retained"), original);
    assert!(installed.join(".ara-managed.json").is_file());
    let project = root.join("project");
    fs::create_dir(&project).expect("current project");
    fs::write(
        project.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"name":"Current SDK proof","layoutVersion":4,"engineVersion":VERSION}),
        ),
    )
    .expect("project marker");
    let project = hub
        .call("hub.register", json!({"path":project}))
        .expect("register current project");
    assert_eq!(
        hub.call(
            "hub.project.currentEngine",
            json!({"project":project["id"],"headless":true})
        )
        .expect("installed capability")["available"],
        true
    );
    let marker_path = installed.join("engine.json");
    let mut marker: Value =
        serde_json::from_slice(&fs::read(&marker_path).expect("managed marker"))
            .expect("marker JSON");
    marker["sourceCommit"] = json!("preview-three-build");
    fs::write(&marker_path, ara_json::to_canonical_string(&marker))
        .expect("old managed identity fixture");
    let before = fs::read(&marker_path).expect("old managed identity");
    let repair = hub
        .call("hub.engine.repair", json!({"version":VERSION}))
        .expect("ordinary signed repair");
    assert_eq!(
        finished(&mut hub, &repair["id"])["state"],
        "failed",
        "ordinary Repair cannot change a pinned source build"
    );
    assert_eq!(
        fs::read(&marker_path).expect("managed original preserved"),
        before
    );
    let replacement = hub
        .call("hub.engine.installCurrent", json!({"version":VERSION}))
        .expect("explicit signed current replacement");
    assert_eq!(finished(&mut hub, &replacement["id"])["state"], "complete");
    let after: Value =
        serde_json::from_slice(&fs::read(&marker_path).expect("new managed identity"))
            .expect("new marker JSON");
    assert_eq!(after["sourceCommit"], "preview-four-build");
    assert_eq!(
        fs::read(&external).expect("external still retained"),
        original
    );
    assert!(
        !installed
            .with_file_name(format!(".repair-backup-{VERSION}"))
            .exists()
    );
}
