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
fn engine_installations_register_durably_and_remove_only_the_matching_build() {
    let root = scratch("installed-engines");
    let mut hub = Hub::open(config(&root)).expect("Hub");
    let installed = engine("0.0.1-alpha");
    hub.call("hub.engine.register", json!({"engine":installed}))
        .expect("register installed build");
    hub.call("hub.engine.register", json!({"engine":installed}))
        .expect("idempotent registration");
    let registry = Registry::load(&root).expect("durable");
    assert_eq!(
        registry
            .engines
            .iter()
            .filter(|e| e.version == installed.version)
            .count(),
        1
    );
    let mut other_path = engine("0.0.1-alpha");
    other_path.executable = root.join("another-installation.exe");
    hub.call("hub.engine.remove", json!({"engine":other_path}))
        .expect("no unrelated removal");
    assert_eq!(
        hub.call("hub.engines", json!({})).expect("engines")["engines"]
            .as_array()
            .expect("list")
            .len(),
        2
    );
    hub.call("hub.engine.remove", json!({"engine":installed}))
        .expect("remove exact build");
    assert_eq!(Registry::load(&root).expect("durable").engines.len(), 1);
    let invalid = EngineBuild {
        version: "future".into(),
        executable: root.join("missing.exe"),
        editor: None,
    };
    assert!(
        hub.call("hub.engine.register", json!({"engine":invalid}))
            .is_err()
    );
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
    // The project marker is an internal `project` document; the template adds one scene.
    let scenes: Vec<&str> = host
        .store()
        .workspace()
        .docs()
        .filter(|(_, doc)| doc.kind == "scene")
        .map(|(path, _)| path)
        .collect();
    assert_eq!(scenes, ["levels/main.scene"]);
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
            ..Template::default()
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

#[test]
fn templates_read_their_marker_and_report_unfetched_sample_art() {
    let root = scratch("sample-lock");
    let templates = root.join("templates");
    let sample = templates.join("outpost");
    fs::create_dir_all(sample.join("evidence")).expect("sample");
    fs::write(
        sample.join("ara.project.json"),
        r#"{"name":"Outpost","main":"levels/outpost.scene","description":"Dusk compound"}"#,
    )
    .expect("marker");
    fs::write(sample.join("evidence/outpost-player.png"), "png").expect("preview");
    fs::write(
        templates.join("assets.lock.json"),
        r#"{"version":1,"assets":[{"id":"barrel","projects":["outpost"],"files":[{"path":"barrel.gltf"},{"path":"barrel.bin"}]}]}"#,
    )
    .expect("lock");
    let source = DirectoryTemplates(templates);
    let outpost = || {
        source
            .list()
            .expect("list")
            .into_iter()
            .find(|t| t.id == "outpost")
            .expect("sample listed")
    };
    let listed = outpost();
    assert_eq!(listed.name, "Outpost");
    assert_eq!(listed.description.as_deref(), Some("Dusk compound"));
    assert_eq!(
        listed.thumbnail,
        Some(sample.join("evidence/outpost-player.png"))
    );
    let setup = listed.setup.expect("art missing");
    assert_eq!(setup.command, "ara-samples fetch outpost");
    assert!(setup.message.contains("2 of 2"), "{}", setup.message);

    let art = sample.join("assets/third_party/barrel");
    fs::create_dir_all(&art).expect("art");
    fs::write(art.join("barrel.gltf"), "{}").expect("gltf");
    fs::write(art.join("barrel.bin"), "").expect("bin");
    assert!(
        outpost().setup.is_none(),
        "fetched art completes the sample"
    );

    // A project created from it keeps the preview as its own `thumbnail.png`.
    let mut config = config(&root.join("config"));
    config.templates = Box::new(DirectoryTemplates(
        sample.parent().expect("templates").into(),
    ));
    let mut hub = Hub::open(config).expect("Hub");
    let created = hub
        .call(
            "hub.create",
            json!({"template":"outpost","name":"Dusk","path":root.join("Dusk")}),
        )
        .expect("create from sample");
    assert!(root.join("Dusk/thumbnail.png").is_file());
    assert!(
        created["thumbnail"]
            .as_str()
            .is_some_and(|p| p.ends_with("thumbnail.png"))
    );
    drop(hub);

    // Evidence is capture output; a new project never copies it.
    let destination = root.join("copy");
    fs::create_dir(&destination).expect("destination");
    source.instantiate("outpost", &destination).expect("copy");
    assert!(
        destination
            .join("assets/third_party/barrel/barrel.bin")
            .is_file()
    );
    assert!(!destination.join("evidence").exists());
}

#[test]
fn create_dry_run_reports_every_field_problem_with_its_field() {
    let root = scratch("dry-run");
    let directory = root.join("config");
    let mut config = config(&directory);
    let templates = directory.join("templates/sample");
    fs::create_dir_all(&templates).expect("templates");
    fs::write(templates.join("ara.project.json"), "{\"name\":\"Sample\"}").expect("marker");
    fs::write(
        directory.join("templates/assets.lock.json"),
        r#"{"assets":[{"id":"art","projects":["sample"],"files":[{"path":"a.png"}]}]}"#,
    )
    .expect("lock");
    config.templates = Box::new(DirectoryTemplates(directory.join("templates")));
    let mut hub = Hub::open(config).expect("Hub");
    fs::create_dir(root.join("Existing")).expect("existing");
    let error = hub
        .call(
            "hub.create",
            json!({"template":"sample","name":" ","path":root.join("Existing"),"engineVersion":"missing","dryRun":true}),
        )
        .expect_err("invalid");
    let fields: Vec<_> = error
        .diagnostics()
        .into_iter()
        .map(|d| (d.location.path.unwrap_or_default(), d.code))
        .collect();
    assert_eq!(
        fields,
        [
            ("name".into(), ara_diag::codes::INVALID_PARAMS),
            ("path".into(), ara_diag::codes::INVALID_PARAMS),
            ("engineVersion".into(), ara_diag::codes::INVALID_PARAMS),
            ("template".into(), ara_diag::codes::TEMPLATE_INCOMPLETE),
        ]
    );
    let ok = hub
        .call(
            "hub.create",
            json!({"template":"blank","name":"Fresh","path":root.join("Fresh"),"dryRun":true}),
        )
        .expect("valid");
    assert_eq!(ok["valid"], true);
    assert!(!root.join("Fresh").exists(), "a dry run creates nothing");
    assert!(
        hub.call("hub.projects", json!({})).expect("projects")["projects"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
}
