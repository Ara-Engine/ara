use super::*;
use crate::{DirectoryTemplates, HubConfig};
use ara_proto::Endpoint;
use std::time::Duration;

fn fixture() -> (PathBuf, Hub) {
    let root = ara_update::testing::scratch("uninstall-preflight");
    let install = root.join("engines/1.0.0");
    fs::create_dir_all(install.join("resources/empty")).expect("isolated install");
    fs::write(install.join(".ara-managed.json"), br#"{"version":"1.0.0"}"#).expect("ownership");
    fs::write(install.join("ara.exe"), b"engine").expect("CLI");
    fs::write(install.join("ara-editor.exe"), b"editor").expect("editor");
    fs::File::create(install.join("resources/large.bin"))
        .expect("sparse fixture")
        .set_len(1024 * 1024)
        .expect("metadata-sized file");
    let mut hub = Hub::open_with_updates(
        HubConfig {
            directory: root.join("config"),
            engine: EngineBuild {
                version: "0.0.1-alpha".into(),
                executable: std::env::current_exe().expect("test executable"),
                editor: None,
            },
            templates: Box::new(DirectoryTemplates(root.join("templates"))),
        },
        UpdateConfig {
            feed: FeedConfig::default(),
            engines: root.join("engines"),
        },
    )
    .expect("isolated Hub");
    hub.call("hub.engine.register", json!({"engine":{
        "version":"1.0.0", "executable":install.join("ara.exe"), "editor":install.join("ara-editor.exe")
    }})).expect("register managed fixture");
    (root, hub)
}

fn finished(hub: &mut Hub, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let jobs = hub.call("hub.downloads", json!({})).expect("job snapshot");
        let job = jobs["jobs"]
            .as_array()
            .expect("jobs")
            .iter()
            .find(|job| job["id"] == id)
            .expect("inspection job");
        if job["state"] != "running" {
            return job.clone();
        }
        assert!(Instant::now() < deadline, "inspection did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn completed_preflight_reports_metadata_size_and_preserves_files_and_registration() {
    let (root, mut hub) = fixture();
    let registry_path = hub.config.directory.join("registry.json");
    let registry = fs::read(&registry_path).expect("registered fixture");
    let install = root.join("engines/1.0.0");
    let expected = 1024 * 1024
        + 12
        + fs::metadata(install.join(".ara-managed.json"))
            .expect("marker metadata")
            .len();
    let started = hub
        .call("hub.engine.uninstall.preflight", json!({"version":"1.0.0"}))
        .expect("preflight");
    assert_eq!(started["kind"], "uninstall-preflight");
    assert_eq!(started["phase"], "inspecting");
    assert_eq!(started["cancellable"], true);
    let job = finished(&mut hub, started["id"].as_str().expect("job ID"));
    assert_eq!(job["state"], "complete", "{job}");
    assert_eq!(
        job["result"],
        json!({"version":"1.0.0","destination":install,"bytes":expected,"files":4})
    );
    assert_eq!(
        fs::read(registry_path).expect("unchanged registry"),
        registry
    );
    assert_eq!(
        fs::read(install.join("ara.exe")).expect("engine remains"),
        b"engine"
    );
    assert!(install.join("resources/empty").is_dir());
    assert!(!root.join("engines/.uninstall-1.0.0").exists());
    assert!(
        hub.registry
            .engines
            .iter()
            .any(|engine| engine.version == "1.0.0")
    );
}

#[test]
fn cancelling_inspection_never_unregisters_or_starts_removal() {
    let (root, mut hub) = fixture();
    let registry_path = hub.config.directory.join("registry.json");
    let registry = fs::read(&registry_path).expect("registered fixture");
    let started = hub
        .begin_uninstall_preflight("1.0.0".into())
        .expect("preflight");
    let id = started["id"].as_str().expect("job ID");
    // Deterministically cover cancellation after the worker may have completed
    // but before its result is consumed by the serialized authority.
    hub.updates.jobs.get(id).expect("job").cancel.cancel();
    let job = finished(&mut hub, id);
    assert_eq!(job["state"], "cancelled", "{job}");
    assert!(job["result"].is_null());
    assert_eq!(fs::read(registry_path).expect("registry remains"), registry);
    assert!(root.join("engines/1.0.0/ara.exe").is_file());
    assert!(!root.join("engines/.uninstall-1.0.0").exists());
    let unrelated_stage = hub.updates.staging.join(id);
    fs::create_dir_all(&unrelated_stage).expect("isolated staging sentinel");
    fs::write(
        unrelated_stage.join("keep.txt"),
        b"not an inspection artifact",
    )
    .expect("sentinel");
    hub.call("hub.download.cancel", json!({"job":id}))
        .expect("cancelled job remains read-only");
    assert!(
        unrelated_stage.join("keep.txt").is_file(),
        "preflight cancellation has no staging cleanup path"
    );
}

#[test]
fn cancelled_metadata_walk_leaves_the_entire_tree_intact() {
    let root = ara_update::testing::scratch("uninstall-inspect-cancelled");
    fs::write(root.join("payload.bin"), b"unchanged").expect("payload");
    let cancel = Cancel::default();
    cancel.cancel();
    assert_eq!(
        inspect(&root, &cancel).expect_err("cancelled").code,
        ara_update::CANCELLED
    );
    assert_eq!(
        fs::read(root.join("payload.bin")).expect("preserved"),
        b"unchanged"
    );
}

#[test]
fn external_registration_and_wrong_ownership_version_are_refused() {
    let (root, mut hub) = fixture();
    let install = root.join("engines/1.0.0");
    fs::write(install.join(".ara-managed.json"), br#"{"version":"2.0.0"}"#)
        .expect("mismatched proof");
    assert!(
        hub.call("hub.engine.uninstall.preflight", json!({"version":"1.0.0"}))
            .is_err()
    );
    assert!(install.join("ara.exe").is_file());
    let external = root.join("external");
    fs::create_dir_all(&external).expect("external root");
    fs::write(external.join("ara.exe"), b"external").expect("external executable");
    hub.registry
        .engines
        .iter_mut()
        .find(|engine| engine.version == "1.0.0")
        .expect("registered")
        .executable = external.join("ara.exe");
    let error = hub
        .call("hub.engine.uninstall.preflight", json!({"version":"1.0.0"}))
        .expect_err("external is registration only");
    assert!(error.message.contains("remove registration"));
    assert_eq!(
        fs::read(external.join("ara.exe")).expect("external preserved"),
        b"external"
    );
}

#[test]
fn current_service_installation_is_refused_before_confirmation() {
    let (root, mut hub) = fixture();
    hub.config.engine.executable = root.join("engines/1.0.0/ara.exe");
    let error = hub
        .call("hub.engine.uninstall.preflight", json!({"version":"1.0.0"}))
        .expect_err("running service");
    assert!(error.message.contains("hosts the running Hub service"));
    assert!(root.join("engines/1.0.0/ara.exe").is_file());
}

#[test]
fn service_path_alias_is_refused_for_inspection_and_confirmed_removal() {
    let (root, mut hub) = fixture();
    let install = root.join("engines/1.0.0");
    // Windows registration simplifies the canonical scratch path's verbatim prefix.
    hub.config.engine.executable = dunce::simplified(&install.join("ara.exe")).to_path_buf();
    let registry = fs::read(hub.config.directory.join("registry.json")).expect("registry");
    for method in ["hub.engine.uninstall.preflight", "hub.engine.uninstall"] {
        let error = hub
            .call(method, json!({"version":"1.0.0"}))
            .expect_err("the current service must stay installed");
        assert!(error.message.contains("hosts the running Hub service"));
    }
    assert!(install.join("ara.exe").is_file());
    assert_eq!(
        fs::read(hub.config.directory.join("registry.json")).expect("unchanged registry"),
        registry
    );
    assert!(hub.updates.jobs.is_empty());
}

#[test]
fn confirmed_removal_accepts_the_registered_managed_path_alias() {
    let (root, mut hub) = fixture();
    let started = hub
        .call("hub.engine.uninstall", json!({"version":"1.0.0"}))
        .expect("registered managed engine");
    let job = finished(&mut hub, started["id"].as_str().expect("job ID"));
    assert_eq!(job["state"], "complete", "{job}");
    assert!(!root.join("engines/1.0.0").exists());
    assert!(hub.registry.engine("1.0.0").is_err());
}

#[test]
#[cfg(windows)]
fn long_service_path_inside_the_installation_is_refused() {
    let (root, mut hub) = fixture();
    let install = root.join("engines/1.0.0");
    let nested = install.join("x".repeat(200));
    fs::create_dir_all(&nested).expect("long owned directory");
    let executable = nested.join("ara.exe");
    assert!(executable.as_os_str().len() > 260);
    fs::write(&executable, b"service").expect("owned service");
    hub.config.engine.executable = executable.clone();
    for method in ["hub.engine.uninstall.preflight", "hub.engine.uninstall"] {
        let error = hub
            .call(method, json!({"version":"1.0.0"}))
            .expect_err("the nested running service must stay installed");
        assert!(error.message.contains("hosts the running Hub service"));
    }
    assert!(executable.is_file());
    assert!(hub.updates.jobs.is_empty());
}

#[test]
#[cfg(unix)]
fn linked_inputs_and_linked_ownership_are_never_followed() {
    use std::os::unix::fs::symlink;
    let root = ara_update::testing::scratch("uninstall-inspect-links");
    let outside = ara_update::testing::scratch("uninstall-inspect-outside");
    fs::write(outside.join("private.bin"), b"outside").expect("outside fixture");
    symlink(&outside, root.join("linked")).expect("linked directory");
    assert!(inspect(&root, &Cancel::default()).is_err());
    assert_eq!(
        fs::read(outside.join("private.bin")).expect("outside untouched"),
        b"outside"
    );
    fs::remove_file(root.join("linked")).expect("remove test link only");
    symlink(outside.join("private.bin"), root.join(".ara-managed.json")).expect("linked ownership");
    assert!(uninstall::validate_managed(&root, "1.0.0").is_err());
}
