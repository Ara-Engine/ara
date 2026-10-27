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

#[test]
fn new_templates_get_balanced_import_policy_and_authored_policy_is_preserved() {
    use ara_asset::{CookPreset, PROJECT_IMPORT_SETTINGS_FILE, ProjectImportSettings};
    let root = scratch("import-policy");
    let templates = DirectoryTemplates(root.join("templates"));
    let blank = root.join("blank");
    fs::create_dir_all(&blank).expect("blank");
    templates.instantiate("blank", &blank).expect("instantiate");
    assert_eq!(
        fs::read_to_string(blank.join(PROJECT_IMPORT_SETTINGS_FILE)).expect("policy"),
        ProjectImportSettings {
            preset: CookPreset::Balanced
        }
        .canonical_text()
        .expect("canonical")
    );
    let custom = templates.0.join("custom");
    fs::create_dir_all(&custom).expect("template");
    fs::write(custom.join("ara.project.json"), "{\"name\":\"Custom\"}").expect("marker");
    let text = ProjectImportSettings {
        preset: CookPreset::HighQuality,
    }
    .canonical_text()
    .expect("canonical");
    fs::create_dir_all(custom.join("settings")).expect("settings");
    fs::write(custom.join(PROJECT_IMPORT_SETTINGS_FILE), &text).expect("policy");
    fs::create_dir_all(custom.join("settings/builds")).expect("build presets");
    fs::write(custom.join("settings/builds/windows.export.json"),"{\"platform\":\"windows\",\"assembly\":\"Custom\",\"content\":\"content\",\"templates\":[]}").expect("authored preset");
    fs::create_dir_all(custom.join("builds/old")).expect("old output");
    fs::write(custom.join("builds/old/player.exe"), "output").expect("output");
    let project = root.join("project");
    fs::create_dir_all(&project).expect("project");
    templates
        .instantiate("custom", &project)
        .expect("instantiate");
    assert_eq!(
        fs::read_to_string(project.join(PROJECT_IMPORT_SETTINGS_FILE)).expect("copy"),
        text
    );
    assert!(
        fs::read_to_string(project.join("settings/builds/windows.export.json"))
            .expect("preset")
            .contains("Custom")
    );
    assert!(!project.join("builds/old/player.exe").exists());
    fs::remove_dir_all(root).expect("scratch cleanup");
}
fn config(directory: &Path) -> HubConfig {
    HubConfig {
        directory: directory.into(),
        engine: engine("alpha"),
        templates: Box::new(DirectoryTemplates(directory.join("templates"))),
    }
}

#[test]
fn names_ignore_case_but_ambiguous_names_require_an_id_or_path() {
    let root = scratch("case-fold").canonicalize().expect("root");
    let mut hub = Hub::open(config(&root.join("config"))).expect("Hub");
    let first = hub
        .call(
            "hub.create",
            json!({"name":"Starter","template":"blank","path":root.join("first")}),
        )
        .expect("first project");
    assert_eq!(
        hub.call("hub.resolve", json!({"project":"sTaRtEr"}))
            .expect("case-insensitive name")["id"],
        first["id"]
    );
    let second = hub
        .call(
            "hub.create",
            json!({"name":"STARTER","template":"blank","path":root.join("second")}),
        )
        .expect("case variant");
    for query in ["starter", "Starter", "STARTER"] {
        let error = hub
            .call("hub.resolve", json!({"project":query}))
            .expect_err("ambiguous name");
        assert!(error.message.contains("ambiguous"), "{error}");
        assert!(
            error
                .message
                .contains(first["id"].as_str().expect("first id")),
            "{error}"
        );
        assert!(
            error
                .message
                .contains(second["id"].as_str().expect("second id")),
            "{error}"
        );
    }
    for project in [first, second] {
        assert_eq!(
            hub.call("hub.resolve", json!({"project":project["id"]}))
                .expect("ID")["id"],
            project["id"]
        );
        assert_eq!(
            hub.call("hub.resolve", json!({"project":project["path"]}))
                .expect("path")["id"],
            project["id"]
        );
    }
    let accented = hub
        .call(
            "hub.create",
            json!({"name":"École","template":"blank","path":root.join("accented")}),
        )
        .expect("Unicode name");
    assert_eq!(
        hub.call("hub.resolve", json!({"project":"éCOLE"}))
            .expect("Unicode lower case")["id"],
        accented["id"]
    );
}

#[test]
fn bootstrap_service_uses_registered_engine_templates_and_their_pins() {
    let root = scratch("bootstrap-templates")
        .canonicalize()
        .expect("scratch");
    let config_dir = root.join("config");
    let service = root.join("service");
    fs::create_dir_all(&service).expect("service");
    fs::write(service.join("ara.exe"), "bootstrap").expect("exe");
    fs::write(service.join("service.json"), "{}").expect("marker");
    let mut hub =
        Hub::open(HubConfig::for_directory(service.join("ara.exe"), config_dir).expect("config"))
            .expect("Hub");
    for version in ["0.0.9", "0.0.10"] {
        let install = root.join(version);
        fs::create_dir_all(install.join("templates/starter")).expect("templates");
        fs::write(install.join("ara.exe"), version).expect("exe");
        fs::write(
            install.join("templates/starter/ara.project.json"),
            r#"{"name":"Starter"}"#,
        )
        .expect("project");
        fs::write(install.join("templates/starter/source.txt"), version).expect("source");
        hub.call("hub.engine.register", json!({"engine":{"version":version,"executable":install.join("ara.exe"),"editor":null}})).expect("register");
    }
    let templates = hub.call("hub.templates", json!({})).expect("templates");
    let starter = templates["templates"]
        .as_array()
        .expect("list")
        .iter()
        .find(|t| t["id"] == "starter")
        .expect("installed Starter, despite no service templates");
    assert_eq!(starter["engineVersion"], "0.0.10");
    for (name, version) in [("newest", None), ("older", Some("0.0.9"))] {
        let mut params = json!({"name":name,"template":"starter","path":root.join(name)});
        if let Some(version) = version {
            params["engineVersion"] = json!(version);
        }
        let created = hub.call("hub.create", params).expect("create");
        let expected = version.unwrap_or("0.0.10");
        assert_eq!(created["engineVersion"], expected);
        assert_eq!(
            fs::read_to_string(root.join(name).join("source.txt")).expect("source"),
            expected
        );
    }
    // The lock beside the selected engine's templates must still be checked.
    fs::write(
        root.join("0.0.10/templates/assets.lock.json"),
        r#"{"assets":[{"id":"art","projects":["starter"],"files":[{"path":"missing.glb"}]}]}"#,
    )
    .expect("lock");
    let error = hub
        .call(
            "hub.create",
            json!({"name":"incomplete","template":"starter","path":root.join("incomplete")}),
        )
        .expect_err("missing locked art");
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code == ara_diag::codes::TEMPLATE_INCOMPLETE)
    );
    assert!(!root.join("incomplete").exists());
}

#[test]
fn engine_installations_register_durably_and_remove_only_the_matching_build() {
    let root = scratch("installed-engines");
    let mut hub = Hub::open(config(&root)).expect("Hub");
    let mut installed = engine("0.0.1-alpha");
    // Canonical Windows paths deliberately include the verbatim prefix.
    installed.executable = fs::canonicalize(&installed.executable).expect("executable");
    installed.editor = Some(installed.executable.clone());
    hub.call("hub.engine.register", json!({"engine":installed}))
        .expect("register installed build");
    hub.call("hub.engine.register", json!({"engine":installed}))
        .expect("idempotent registration");
    let registry = Registry::load(&root).expect("durable");
    let expected = ara_hub_service::normalized_path(&installed.executable).to_owned();
    let persisted: Value =
        serde_json::from_slice(&fs::read(root.join("registry.json")).expect("persisted registry"))
            .expect("registry JSON");
    assert_eq!(persisted["engines"][1]["executable"], json!(expected));
    assert_eq!(persisted["engines"][1]["editor"], json!(expected));
    assert_eq!(registry.engines[1].executable, expected);
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
    drop(hub);
    let mut legacy = Registry::default();
    legacy.engines.push(installed);
    fs::write(
        root.join("registry.json"),
        ara_json::to_canonical_string(&serde_json::to_value(&legacy).expect("legacy registry")),
    )
    .expect("old verbatim registry");
    let reloaded = Registry::load(&root).expect("load old registry");
    assert_eq!(reloaded.engines[0].executable, expected);
    assert_eq!(
        reloaded.engines[0].editor.as_deref(),
        Some(expected.as_path())
    );
}

#[test]
fn create_makes_missing_parents_but_never_reuses_a_target() {
    let root = scratch("create-parents");
    let mut hub = Hub::open(config(&root.join("config"))).expect("Hub");
    let path = root.join("missing/nested/Project");
    let params = json!({"name":"Nested", "template":"blank", "path":path});
    let mut dry_run = params.clone();
    dry_run["dryRun"] = json!(true);
    hub.call("hub.create", dry_run)
        .expect("valid missing parents");
    assert!(
        !root.join("missing").exists(),
        "dry runs never create folders"
    );
    hub.call("hub.create", params.clone())
        .expect("mkdir-p parents");
    assert!(path.join("ara.project.json").is_file());
    fs::write(path.join("keep.txt"), "preserved").expect("sentinel");
    let mut duplicate = params;
    duplicate["name"] = json!("Different");
    assert!(
        hub.call("hub.create", duplicate).is_err(),
        "existing target"
    );
    assert_eq!(
        fs::read_to_string(path.join("keep.txt")).expect("sentinel"),
        "preserved"
    );
    fs::write(root.join("file"), "not a directory").expect("file parent");
    assert!(hub.call("hub.create", json!({"name":"Blocked", "template":"blank", "path":root.join("file/nested/project"),"dryRun":true})).is_err());
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
    assert_eq!(
        scenes,
        [format!(
            "{}/scenes/main.scene",
            ara_hub_service::AUTHORING_ROOT
        )]
    );
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
    assert!(
        !destination.join(".ara/token").exists(),
        "template credentials must never be copied"
    );
    assert!(
        fs::read_dir(destination.join(".ara"))
            .expect("migration state")
            .all(|entry| entry.expect("state entry").file_name() == "lock")
    );
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
            .join(ara_hub_service::AUTHORING_ROOT)
            .join("third_party/barrel/barrel.bin")
            .is_file()
    );
    assert!(!destination.join("evidence").exists());
}

#[test]
fn missing_generated_kits_block_creation_even_after_third_party_art_is_fetched() {
    let root = scratch("missing-kits");
    let templates = root.join("templates");
    let sample = templates.join("outpost");
    let kit = sample.join("assets/kit");
    fs::create_dir_all(&kit).expect("kit descriptors");
    fs::create_dir_all(sample.join("assets/third_party/art")).expect("fetched art");
    fs::write(sample.join("assets/third_party/art/palette.png"), "image").expect("art");
    fs::write(sample.join("ara.project.json"), r#"{"name":"Outpost"}"#).expect("marker");
    fs::write(
        templates.join("assets.lock.json"),
        r#"{"assets":[{"id":"art","projects":["outpost"],"files":[{"path":"palette.png"}]}]}"#,
    )
    .expect("lock");
    for name in ["mesh.gltf", "palette.png"] {
        fs::write(kit.join(format!("{name}.import.json")), "{}").expect("descriptor");
    }
    let mut settings = config(&root.join("config"));
    settings.templates = Box::new(DirectoryTemplates(templates.clone()));
    let mut hub = Hub::open(settings).expect("Hub");
    let setup = hub.call("hub.templates", json!({})).expect("templates")["templates"]
        .as_array()
        .expect("tiles")
        .iter()
        .find(|t| t["id"] == "outpost")
        .expect("Outpost")["setup"]
        .clone();
    assert_eq!(setup["command"], "cargo run -p ara-samples -- kits outpost");
    assert!(
        setup["message"]
            .as_str()
            .is_some_and(|s| s.contains("mesh.gltf"))
    );
    let destination = root.join("Created");
    for dry_run in [true, false] {
        let error = hub
            .call(
                "hub.create",
                json!({"template":"outpost","name":"Created","path":destination,"dryRun":dry_run}),
            )
            .expect_err("missing kits");
        let diagnostic = error.diagnostics().into_iter().next().expect("diagnostic");
        assert_eq!(diagnostic.code, ara_diag::codes::TEMPLATE_INCOMPLETE);
        assert_eq!(diagnostic.location.path.as_deref(), Some("template"));
        assert!(
            diagnostic
                .message
                .contains("cargo run -p ara-samples -- kits outpost")
        );
        assert!(
            !destination.exists(),
            "refuse before creating a broken project"
        );
    }
    assert!(
        hub.call("hub.projects", json!({})).expect("projects")["projects"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
    let direct = root.join("Direct");
    fs::create_dir(&direct).expect("empty destination");
    DirectoryTemplates(templates.clone())
        .instantiate("outpost", &direct)
        .expect_err("direct copying must also reject missing kits");
    assert_eq!(fs::read_dir(&direct).expect("destination").count(), 0);

    // A partial cook can leave the glTF and texture while its binary is absent.
    fs::write(kit.join("palette.png"), "image").expect("generated palette");
    fs::write(kit.join("mesh.gltf"), r#"{"buffers":[{"uri":"mesh.bin"},{"uri":"data:application/octet-stream;base64,AA=="}],"images":[{"uri":"palette.png"},{"uri":"../third_party/art/palette.png"}]}"#).expect("generated mesh");
    let source = DirectoryTemplates(templates);
    let incomplete = source
        .list()
        .expect("list")
        .into_iter()
        .find(|t| t.id == "outpost")
        .expect("Outpost")
        .setup
        .expect("missing binary");
    assert!(incomplete.message.contains("mesh.bin"));
    fs::write(kit.join("mesh.bin"), [0]).expect("generated buffer");
    assert!(
        source
            .list()
            .expect("list")
            .into_iter()
            .find(|t| t.id == "outpost")
            .expect("Outpost")
            .setup
            .is_none()
    );
    hub.call(
        "hub.create",
        json!({"template":"outpost","name":"Created","path":destination}),
    )
    .expect("complete generated kits");
    assert!(
        destination
            .join(ara_hub_service::AUTHORING_ROOT)
            .join("kit/mesh.bin")
            .is_file()
    );
    assert!(
        destination
            .join(ara_hub_service::AUTHORING_ROOT)
            .join("kit/palette.png")
            .is_file()
    );
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
