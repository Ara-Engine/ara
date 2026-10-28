//! Migration previews never open a project authority or mutate authored files.
#[path = "support/capabilities.rs"]
mod support;
use ara_hub_service::{ALPHA_VERSION, DirectoryTemplates, EngineBuild, Hub, HubConfig};
use ara_proto::ConnId;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

struct Fixture {
    _scratch: tempfile::TempDir,
    hub: Hub,
    project: String,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::var_os("ARA_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    PathBuf::from("D:/AraBuild/ux-hub-evidence")
                } else {
                    std::env::temp_dir()
                }
            })
            .join("hub-migration-tests");
        fs::create_dir_all(&base).expect("isolated test root");
        let scratch = tempfile::tempdir_in(base).expect("scratch project/profile");
        let root = scratch.path().join("old-project");
        fs::create_dir_all(root.join("levels")).expect("legacy scene directory");
        fs::write(
            root.join("levels/main.scene"),
            r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{}}"#,
        )
        .expect("legacy scene");
        fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&json!({"name":"Legacy fixture","main":"levels/main.scene","engineVersion":ALPHA_VERSION}))).expect("legacy marker");
        let mut hub = Hub::open(HubConfig {
            directory: scratch.path().join("isolated-profile"),
            engine: EngineBuild {
                version: ALPHA_VERSION.into(),
                executable: support::engine_cli(&scratch.path().join("current-engine"), Some(&[4])),
                editor: None,
            },
            templates: Box::new(DirectoryTemplates(scratch.path().join("templates"))),
        })
        .expect("isolated Hub");
        let registered = hub
            .call_for(ConnId::LOCAL, "hub.register", json!({"path":root}))
            .expect("register legacy project");
        let project = registered["id"].as_str().expect("project ID").to_owned();
        Self {
            _scratch: scratch,
            hub,
            project,
            root,
        }
    }

    fn call(&mut self, method: &str, args: Value) -> Value {
        let mut args = args;
        args["project"] = json!(self.project);
        self.hub
            .call_for(ConnId::LOCAL, method, args)
            .unwrap_or_else(|error| panic!("{method}: {error}"))
    }

    fn complete(&mut self, job: Value) -> Value {
        let id = job["id"].as_str().expect("job identifier").to_owned();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = self.call("hub.project.migration.status", json!({"job":id}));
            if status["state"] != "running" {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "migration worker timed out: {status}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn old_open_is_guarded_and_cancelled_preview_has_no_project_side_effects() {
    let mut fixture = Fixture::new();
    let marker = fs::read(fixture.root.join("ara.project.json")).expect("before marker");
    let scene = fs::read(fixture.root.join("levels/main.scene")).expect("before scene");
    let error = fixture
        .hub
        .call_for(
            ConnId::LOCAL,
            "hub.open",
            json!({"project":fixture.project,"headless":true}),
        )
        .expect_err("old normal authority must not start");
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == ara_diag::codes::PROJECT_MIGRATION_REQUIRED)
    );
    for (method, params) in [
        ("hub.catalog", json!({"project":fixture.project})),
        ("hub.activity", json!({"project":fixture.project})),
        (
            "hub.call",
            json!({"project":fixture.project,"method":"rpc.discover","params":{}}),
        ),
    ] {
        let error = fixture
            .hub
            .call_for(ConnId::LOCAL, method, params)
            .expect_err("old project attach is gated before a downstream client");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == ara_diag::codes::PROJECT_MIGRATION_REQUIRED),
            "{method}: {error}"
        );
    }
    fixture
        .hub
        .call_for(ConnId::LOCAL, "hub.catalog", json!({}))
        .expect("Hub internal metadata catalog does not need a project layout");
    assert!(
        !fixture.root.join(".ara").exists(),
        "the guard precedes daemon/profile initialization"
    );
    let job = fixture.call("hub.project.migration.plan", json!({}));
    fixture.call("hub.project.migration.cancel", json!({"job":job["id"]}));
    let status = fixture.complete(job);
    assert!(
        matches!(status["state"].as_str(), Some("complete" | "cancelled")),
        "readonly cancellation result: {status}"
    );
    assert_eq!(
        fs::read(fixture.root.join("ara.project.json")).expect("unchanged marker"),
        marker
    );
    assert_eq!(
        fs::read(fixture.root.join("levels/main.scene")).expect("unchanged scene"),
        scene
    );
    assert!(!fixture.root.join(".ara").exists());
    assert!(!fixture.root.join("content").exists());
}

#[test]
fn only_explicit_migration_publishes_the_current_layout_after_the_receipt() {
    let mut fixture = Fixture::new();
    let job = fixture.call("hub.project.migration.plan", json!({}));
    let preview = fixture.complete(job);
    assert_eq!(preview["state"], "complete", "{preview}");
    let plan = &preview["result"];
    assert_eq!(plan["needsMigration"], true);
    assert_eq!(plan["authoredRoot"], "content");
    assert!(
        !fixture.root.join("content").exists(),
        "preview cannot prepare authored destinations"
    );
    let started = fixture.call(
        "hub.project.migrate",
        json!({"planToken":plan["planToken"]}),
    );
    let result = fixture.complete(started);
    assert_eq!(result["state"], "complete", "{result}");
    ara_host::project_layout::require_current_layout(&fixture.root)
        .expect("physical migration completed");
    assert!(fixture.root.join("content/levels/main.scene").is_file());
    assert!(!fixture.root.join("levels/main.scene").exists());
    assert!(
        fixture.root.join(".ara/journal").is_file(),
        "typed migration is journalled"
    );
    let card = fixture.call("hub.resolve", json!({}));
    assert_eq!(
        card["engineVersion"],
        ara_host::project_layout::CURRENT_ENGINE_VERSION,
        "migration refreshes the registry from its physically saved engine pin"
    );
}

#[test]
fn content_project_refuses_undeclared_engine_before_launch_and_typed_choice_refreshes_registry() {
    let mut fixture = Fixture::new();
    fs::create_dir(fixture.root.join("content")).expect("current fixture content");
    fs::rename(
        fixture.root.join("levels"),
        fixture.root.join("content/levels"),
    )
    .expect("current fixture scene");
    let mut marker: Value = serde_json::from_slice(
        &fs::read(fixture.root.join("ara.project.json")).expect("fixture marker"),
    )
    .expect("fixture JSON");
    marker["layoutVersion"] = json!(4);
    fs::write(
        fixture.root.join("ara.project.json"),
        ara_json::to_canonical_string(&marker),
    )
    .expect("current fixture marker");
    let old = EngineBuild {
        version: "preview-3".into(),
        executable: std::env::current_exe().expect("undeclared executable"),
        editor: None,
    };
    fixture
        .hub
        .call_for(ConnId::LOCAL, "hub.engine.register", json!({"engine":old}))
        .expect("old registration");
    fixture.call("hub.update", json!({"engineVersion":"preview-3"}));
    let before = fs::read(fixture.root.join("ara.project.json")).expect("old pin");
    let error = fixture
        .hub
        .call_for(
            ConnId::LOCAL,
            "hub.open",
            json!({"project":fixture.project,"headless":true}),
        )
        .expect_err("v4 cannot launch an undeclared v3 engine");
    assert!(error.diagnostics().iter().any(|diagnostic|diagnostic.code==ara_diag::codes::PROJECT_ENGINE_LAYOUT_UNSUPPORTED));
    assert_eq!(
        fs::read(fixture.root.join("ara.project.json")).expect("unchanged refused pin"),
        before
    );
    assert_eq!(fixture.call("hub.status", json!({}))["state"], "closed");
    assert_eq!(
        fixture.call("hub.project.currentEngine", json!({"headless":true}))["available"],
        true
    );
    let job = fixture.call("hub.project.useCurrentEngine", json!({"headless":true}));
    assert_eq!(job["kind"], "pin");
    assert_eq!(job["cancellable"], false);
    let result = fixture.complete(job);
    assert_eq!(result["state"], "complete", "{result}");
    let card = fixture.call("hub.resolve", json!({}));
    assert_eq!(
        card["engineVersion"],
        ara_host::project_layout::CURRENT_ENGINE_VERSION
    );
    let marker: Value = serde_json::from_slice(
        &fs::read(fixture.root.join("ara.project.json")).expect("saved marker"),
    )
    .expect("marker JSON");
    assert_eq!(marker["engineVersion"], card["engineVersion"]);
    let persisted =
        ara_hub_service::Registry::load(&fixture._scratch.path().join("isolated-profile"))
            .expect("saved registry");
    assert_eq!(
        persisted
            .resolve(&fixture.project)
            .expect("persisted card")
            .engine_version,
        ara_host::project_layout::CURRENT_ENGINE_VERSION
    );
}

#[test]
fn undeclared_engine_does_not_claim_legacy_root_layout_support() {
    let mut fixture = Fixture::new();
    let marker = fs::read(fixture.root.join("ara.project.json")).expect("before marker");
    fixture
        .hub
        .call_for(
            ConnId::LOCAL,
            "hub.engine.register",
            json!({"engine":EngineBuild {
                version: ALPHA_VERSION.into(),
                executable: std::env::current_exe().expect("undeclared executable"),
                editor: None,
            }}),
        )
        .expect("undeclared engine");
    let error = fixture
        .hub
        .call_for(
            ConnId::LOCAL,
            "hub.catalog",
            json!({"project":fixture.project}),
        )
        .expect_err("v3-only engines cannot claim legacy root layout support");
    assert!(error.diagnostics().iter().any(|diagnostic| diagnostic.code == ara_diag::codes::PROJECT_MIGRATION_REQUIRED));
    assert_eq!(
        fs::read(fixture.root.join("ara.project.json")).expect("untouched marker"),
        marker
    );
    assert!(fixture.root.join("levels/main.scene").is_file());
    assert!(!fixture.root.join("content").exists());
    assert!(!fixture.root.join(".ara").exists());
}
