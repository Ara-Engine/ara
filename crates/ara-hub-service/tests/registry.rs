//! Durable registry, replaceable templates and single-instance ownership.
use ara_hub_service::{
    DirectoryTemplates, EngineBuild, Hub, HubConfig, Registry, Template, TemplateSource,
};
use ara_proto::{Endpoint, RpcError};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

fn scratch(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/hub-service-tests")
        .join(format!("{}-{name}-{stamp}", std::process::id()));
    fs::create_dir_all(&path).expect("scratch");
    path
}
fn engine(version: &str) -> EngineBuild {
    EngineBuild {
        version: version.into(),
        executable: std::env::current_exe().expect("exe"),
        editor: None,
    }
}
fn config(directory: &Path) -> HubConfig {
    HubConfig {
        directory: directory.into(),
        engine: engine("alpha"),
        templates: Box::new(DirectoryTemplates(directory.join("templates"))),
    }
}

#[test]
fn canonical_registry_survives_restart_and_pins_the_project() {
    let root = scratch("registry");
    let config_dir = root.join("config");
    let mut hub = Hub::open(config(&config_dir)).expect("Hub");
    assert!(Hub::open(config(&config_dir)).is_err(), "single instance");
    let created = hub
        .call(
            "hub.create",
            json!({"name":"Example","template":"blank","path":root.join("Example")}),
        )
        .expect("create");
    let marker: Value =
        serde_json::from_slice(&fs::read(root.join("Example/ara.project.json")).expect("marker"))
            .expect("JSON");
    assert_eq!(marker["engineVersion"], "alpha");
    let project = ara_host::Project::open(&root.join("Example")).expect("valid project");
    let host = ara_host::Host::open(project, Default::default()).expect("valid template scene");
    assert_eq!(host.store().workspace().docs().count(), 1);
    drop(host);
    hub.call(
        "hub.update",
        json!({"project":created["id"],"name":"Renamed"}),
    )
    .expect("rename");
    drop(hub);
    let registry = Registry::load(&config_dir).expect("reload");
    assert_eq!(registry.resolve("Renamed").expect("name").id, created["id"]);
    assert_eq!(
        registry
            .resolve(root.join("Example").to_str().expect("path"))
            .expect("path")
            .name,
        "Renamed"
    );
    assert!(registry.resolve("invisible").is_err());
    let bytes = fs::read_to_string(config_dir.join("registry.json")).expect("saved registry");
    assert_eq!(
        bytes,
        ara_json::to_canonical_string(&serde_json::from_str(&bytes).expect("registry JSON"))
    );
    fs::write(config_dir.join("registry.json"), "invalid").expect("corrupt");
    assert!(
        Registry::load(&config_dir).is_err(),
        "never erase a corrupt registry"
    );
}

#[test]
fn template_directory_copies_sources_and_excludes_machine_state() {
    let root = scratch("templates");
    let templates = root.join("templates");
    let sample = templates.join("sample");
    fs::create_dir_all(&sample).expect("sample");
    fs::write(sample.join("ara.project.json"), "{\"name\":\"Sample\"}").expect("marker");
    fs::create_dir(sample.join(".ara")).expect("state");
    fs::write(sample.join(".ara/token"), "private").expect("token");
    fs::write(sample.join("lesson.txt"), "Learn Ara").expect("lesson");
    let source = DirectoryTemplates(templates);
    let list = source.list().expect("list");
    assert_eq!(
        list.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["blank", "sample"]
    );
    let destination = root.join("copy");
    fs::create_dir(&destination).expect("destination");
    source.instantiate("sample", &destination).expect("copy");
    assert!(destination.join("lesson.txt").is_file());
    assert!(!destination.join(".ara").exists());
    assert!(source.instantiate("../sample", &destination).is_err());
}

struct CustomSource;
impl TemplateSource for CustomSource {
    fn list(&self) -> Result<Vec<Template>, RpcError> {
        Ok(vec![Template {
            id: "custom".into(),
            name: "Packaged sample".into(),
            thumbnail: None,
        }])
    }
    fn instantiate(&self, _: &str, destination: &Path) -> Result<(), RpcError> {
        fs::write(
            destination.join("ara.project.json"),
            "{\"name\":\"Packaged\"}",
        )
        .expect("marker");
        Ok(())
    }
}
#[test]
fn template_sources_are_pluggable_and_engine_versions_coexist() {
    let root = scratch("pluggable");
    let directory = root.join("config");
    fs::create_dir(&directory).expect("config");
    let mut registry = Registry::default();
    registry.engines.push(engine("older"));
    registry.save(&directory).expect("save");
    let mut config = config(&directory);
    config.templates = Box::new(CustomSource);
    let mut hub = Hub::open(config).expect("Hub");
    assert_eq!(
        hub.call("hub.engines", json!({})).expect("engines")["engines"]
            .as_array()
            .expect("engines")
            .len(),
        2
    );
    let created = hub.call("hub.create",json!({"template":"custom","name":"Custom","path":root.join("Custom"),"engineVersion":"older"})).expect("custom source");
    assert_eq!(created["engineVersion"], "older");
    assert!(
        hub.call(
            "hub.update",
            json!({"project":created["id"],"engineVersion":"missing"})
        )
        .is_err()
    );
}
