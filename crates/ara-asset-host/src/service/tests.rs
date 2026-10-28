//! Disposable authority tests; production workers never touch the user's profile.
use super::{
    AssetService, bindings::Bindings, import_entries::entry_metadata, inventory::Inventory,
};
use ara_asset::{Cancellation, ProcessWorker};
use ara_host::{Host, HostConfig, Project};
use ara_proto::ConnId;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) fn scratch() -> tempfile::TempDir {
    let root = if cfg!(windows) {
        std::path::PathBuf::from("D:/AraBuild/ux-hub-evidence/asset-service-tests")
    } else {
        std::env::temp_dir()
    };
    fs::create_dir_all(&root).expect("test scratch root");
    tempfile::Builder::new()
        .prefix("ara-asset-service-")
        .tempdir_in(root)
        .expect("isolated scratch")
}

pub(super) fn write_project(root: &Path) {
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    fs::create_dir_all(authored.join("scenes")).expect("scene directory");
    fs::create_dir_all(authored.join("props")).expect("props directory");
    fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(&json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Asset service test","main":"scenes/main.scene"})),
    )
    .expect("marker");
    fs::write(
        authored.join("scenes/main.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{}}"#,
    )
    .expect("scene");
    fs::write(authored.join("props/notes.txt"), "immutable authored bytes").expect("source");
}

pub(super) fn bindings(root: &Path) -> Bindings {
    let project = Project::open(root).expect("project");
    let (workspace, diagnostics) = project.load(&[]);
    assert!(
        diagnostics
            .values()
            .all(|diagnostics| !diagnostics.iter().any(ara_diag::Diagnostic::is_error)),
        "{diagnostics:?}"
    );
    Bindings {
        schema: project.db,
        workspace,
        versions: Default::default(),
        preset: Default::default(),
        asset_hashes: Default::default(),
    }
}

pub(super) fn open(root: &Path) -> Host {
    let mut host = Host::open(
        Project::open(root).expect("project"),
        HostConfig {
            debounce: Duration::from_secs(3600),
            watch_every: Duration::from_secs(3600),
            scan_every: Duration::from_secs(3600),
            seed: Some(941),
            ..HostConfig::default()
        },
    )
    .expect("host");
    host.register(Box::new(AssetService::new(
        ProcessWorker::new(root.join("must-not-run-importer")),
        Arc::new(|_, _| Ok(())),
    )))
    .expect("asset service");
    host
}

pub(super) fn call(host: &mut Host, method: &str, params: Value) -> Value {
    host.call(ConnId::LOCAL, method, params)
        .unwrap_or_else(|error| panic!("{method}: {error}"))
}

pub(super) fn begin(host: &mut Host, scopes: Option<Value>) -> String {
    let mut params = json!({"client":"asset-service-test","author":"human"});
    if let Some(scopes) = scopes {
        params["scopes"] = scopes;
    }
    call(host, "session.begin", params)["session"]
        .as_str()
        .expect("session")
        .to_owned()
}

pub(super) fn complete(host: &mut Host, session: &str, job: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.tick();
        let status = call(host, "jobs.status", json!({"session":session,"job":job}));
        match status["status"].as_str() {
            Some("done") => return status["result"].clone(),
            Some("failed") => panic!("asset job failed: {status}"),
            Some("cancelled") => panic!("asset job unexpectedly cancelled: {status}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "asset job timed out: {status}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn readonly_inventory_hashes_without_creating_history() {
    let scratch = scratch();
    write_project(scratch.path());
    let bindings = bindings(scratch.path());
    let inventory = Inventory::read(scratch.path(), &bindings, true, &Cancellation::default())
        .expect("readonly inventory");
    assert!(
        inventory
            .prepared
            .entries
            .contains_key(&format!("{}/props/notes.txt", ara_doc::AUTHORING_ROOT))
    );
    assert!(
        !scratch.path().join(".ara").exists(),
        "read-only preflight created derived data"
    );
    fs::write(
        scratch
            .path()
            .join(ara_doc::AUTHORING_ROOT)
            .join("props/notes.txt"),
        "external edit",
    )
    .expect("external edit");
    assert!(
        ara_host::file_state::FileHistory::new(scratch.path())
            .revalidate(&inventory.prepared)
            .is_err()
    );
    assert!(!scratch.path().join(".ara").exists());
}

#[test]
fn native_preflight_decodes_checked_bytes_without_staging_or_history() {
    let scratch = scratch();
    write_project(scratch.path());
    let bindings = bindings(scratch.path());
    let inventory = Inventory::read(scratch.path(), &bindings, true, &Cancellation::default())
        .expect("readonly inventory");
    let selected = scratch.path().join("incoming.scene");
    let bytes = br#"{"$schema":"ara://schema/scene/1","id":"sc_foreign","records":{}}"#;
    fs::write(&selected, bytes).expect("incoming");
    let entry = crate::import::Entry {
        source: Some(selected.clone()),
        source_reference: "incoming.scene".into(),
        destination: "scenes/incoming.scene".into(),
        kind: crate::import::EntryKind::File,
        disposition: crate::import::Disposition::Created,
        bytes: bytes.len() as u64,
        digest: Some(ara_asset::Digest::of(bytes)),
        staged: None,
        asset_id: None,
        native_document: true,
    };
    let decoded = entry_metadata(
        scratch.path(),
        &bindings,
        &inventory,
        &entry,
        true,
        &Cancellation::default(),
    )
    .expect("native readonly decode");
    assert_eq!(
        decoded.document.expect("native document").id.as_str(),
        "sc_foreign"
    );
    assert!(!scratch.path().join(".ara").exists());
    fs::write(selected, "malformed incoming source").expect("changed source");
    assert!(
        entry_metadata(
            scratch.path(),
            &bindings,
            &inventory,
            &entry,
            true,
            &Cancellation::default()
        )
        .is_err(),
        "changed incoming bytes accepted"
    );
    assert!(!scratch.path().join(".ara").exists());
}

#[test]
fn external_read_permission_is_checked_before_worker_or_filesystem_reads() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut host = open(scratch.path());
    let session = begin(&mut host, Some(json!(["read", "edit:assets", "import"])));
    let result = host.call(ConnId::LOCAL, "cmd.apply", json!({"session":session,"commands":[{"cmd":"asset.import","args":{"sources":[scratch.path().join("private-missing").to_string_lossy()],"destination":"props","dryRun":true}}]})).expect_err("outside-project permission required");
    let diagnostics = result.diagnostics();
    assert!(
        diagnostics.iter().any(
            |diagnostic| diagnostic.code == ara_diag::codes::MISSING_SCOPE
                && diagnostic.message.contains("fs:outside-project")
        ),
        "{diagnostics:?}"
    );
    assert!(!scratch.path().join(".ara/file-history").exists());
    assert_eq!(
        call(&mut host, "jobs.list", json!({"session":session}))["jobs"]
            .as_array()
            .expect("jobs")
            .len(),
        0
    );
}

#[test]
fn async_move_finishes_after_physical_files_and_retains_undo_bytes() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut host = open(scratch.path());
    let session = begin(&mut host, None);
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"why":"Organize notes","idempotencyKey":"notes-move","commands":[{"cmd":"asset.move","args":{"source":"props/notes.txt","destination":"props/moved.txt"}}]}),
    );
    let job = started["job"].as_str().expect("background job");
    let same_pending = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"idempotencyKey":"notes-move","commands":[{"cmd":"asset.move","args":{"source":"props/notes.txt","destination":"props/moved.txt"}}]}),
    );
    assert_eq!(
        same_pending["job"], started["job"],
        "retry started another preparation job before physical completion"
    );
    complete(&mut host, &session, job);
    let authored = scratch.path().join(ara_doc::AUTHORING_ROOT);
    assert!(!authored.join("props/notes.txt").exists());
    assert_eq!(
        fs::read(authored.join("props/moved.txt")).expect("materialized destination"),
        b"immutable authored bytes"
    );
    let replay = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"idempotencyKey":"notes-move","commands":[{"cmd":"asset.move","args":{"source":"props/notes.txt","destination":"props/moved.txt"}}]}),
    );
    assert_eq!(
        replay["replayed"], true,
        "idempotent retry prepared the missing old source"
    );
    let undo = call(&mut host, "history.undo", json!({"session":session}));
    if let Some(job) = undo["job"].as_str() {
        complete(&mut host, &session, job);
    }
    host.flush();
    assert_eq!(
        fs::read(authored.join("props/notes.txt")).expect("restored source"),
        b"immutable authored bytes"
    );
    assert!(!authored.join("props/moved.txt").exists());
}

#[test]
fn cancellation_before_first_tick_never_commits_prepared_files() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut host = open(scratch.path());
    let session = begin(&mut host, None);
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"commands":[{"cmd":"asset.move","args":{"source":"props/notes.txt","destination":"props/cancelled.txt"}}]}),
    );
    let job = started["job"].as_str().expect("job");
    call(
        &mut host,
        "jobs.cancel",
        json!({"session":session,"job":job}),
    );
    for _ in 0..8 {
        host.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        call(
            &mut host,
            "jobs.status",
            json!({"session":session,"job":job})
        )["status"],
        "cancelled"
    );
    assert!(
        scratch
            .path()
            .join(ara_doc::AUTHORING_ROOT)
            .join("props/notes.txt")
            .exists()
    );
    assert!(
        !scratch
            .path()
            .join(ara_doc::AUTHORING_ROOT)
            .join("props/cancelled.txt")
            .exists()
    );
}

#[test]
fn multi_asset_batch_and_folder_creation_share_one_transaction_and_undo() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut host = open(scratch.path());
    let session = begin(&mut host, None);
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"why":"Organize one asset selection","commands":[
            {"cmd":"folder.create","args":{"path":"warehouse/empty","parents":true}},
            {"cmd":"asset.move","args":{"source":"props/notes.txt","destination":"warehouse/notes.txt"}},
            {"cmd":"asset.duplicate","args":{"source":"warehouse/notes.txt","destination":"warehouse/copy.txt"}},
            {"cmd":"asset.rename","args":{"source":"warehouse/copy.txt","name":"backup.txt"}}
        ]}),
    );
    complete(
        &mut host,
        &session,
        started["job"].as_str().expect("one preparation job"),
    );
    let authored = scratch.path().join(ara_doc::AUTHORING_ROOT);
    assert!(authored.join("warehouse/empty").is_dir());
    assert_eq!(
        fs::read(authored.join("warehouse/notes.txt")).expect("moved bytes"),
        b"immutable authored bytes"
    );
    assert_eq!(
        fs::read(authored.join("warehouse/backup.txt")).expect("copied bytes"),
        b"immutable authored bytes"
    );
    let history = call(&mut host, "history.list", json!({"session":session}));
    assert_eq!(history["items"].as_array().expect("transactions").len(), 1);
    let undo = call(&mut host, "history.undo", json!({"session":session}));
    if let Some(job) = undo["job"].as_str() {
        complete(&mut host, &session, job);
    }
    host.flush();
    assert_eq!(
        fs::read(authored.join("props/notes.txt")).expect("restored source"),
        b"immutable authored bytes"
    );
    assert!(!authored.join("warehouse").exists());
}

#[test]
fn an_ended_initiating_session_cannot_commit_later() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut host = open(scratch.path());
    let session = begin(&mut host, None);
    let observer = begin(&mut host, None);
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"commands":[{"cmd":"asset.move","args":{"source":"props/notes.txt","destination":"props/expired.txt"}}]}),
    );
    call(&mut host, "session.end", json!({"session":session}));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.tick();
        let status = call(
            &mut host,
            "jobs.status",
            json!({"session":observer,"job":started["job"]}),
        );
        if status["status"] == "failed" {
            assert!(
                status["diagnostics"]
                    .as_array()
                    .expect("diagnostics")
                    .iter()
                    .any(|diagnostic| diagnostic["code"]
                        == ara_diag::codes::SESSION_NOT_FOUND.to_string())
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expired-session job did not fail: {status}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        scratch
            .path()
            .join(ara_doc::AUTHORING_ROOT)
            .join("props/notes.txt")
            .exists()
    );
    assert!(
        !scratch
            .path()
            .join(ara_doc::AUTHORING_ROOT)
            .join("props/expired.txt")
            .exists()
    );
}

#[test]
fn concurrent_authority_edit_rejects_prepared_asset_batch() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut host = open(scratch.path());
    let session = begin(&mut host, None);
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"commands":[{"cmd":"asset.move","args":{"source":"props/notes.txt","destination":"props/stale.txt"}}]}),
    );
    call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"commands":[{"cmd":"doc.create","args":{"path":"scenes/concurrent.scene","kind":"scene","id":"sc_concurrent"}}]}),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.tick();
        let status = call(
            &mut host,
            "jobs.status",
            json!({"session":session,"job":started["job"]}),
        );
        if status["status"] == "failed" {
            assert!(
                status["diagnostics"]
                    .as_array()
                    .expect("diagnostics")
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == ara_diag::codes::STALE_BASE.to_string())
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stale asset batch did not fail: {status}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        scratch
            .path()
            .join(ara_doc::AUTHORING_ROOT)
            .join("props/notes.txt")
            .exists()
    );
    assert!(
        !scratch
            .path()
            .join(ara_doc::AUTHORING_ROOT)
            .join("props/stale.txt")
            .exists()
    );
}

#[test]
fn older_sidecar_uses_installed_discovery_version_without_rewriting_authority() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut bindings = bindings(scratch.path());
    ara_asset::register_schema(&mut bindings.schema);
    ara_importers::registry()
        .expect("builtins")
        .register_schemas(&mut bindings.schema);
    bindings
        .versions
        .insert("ara.texture".into(), "4-current".into());
    let sidecar = ara_asset::Sidecar::read(r#"{"$schema":"ara://schema/import/1","id":"as_source","records":{"i_settings":{"importer":"ara.texture","version":"3-old","settings":{}}}}"#, "texture.png.import.json", &bindings.schema).expect("old supported sidecar").into_document();
    bindings.workspace.insert(
        bindings.workspace.storage_path("texture.png.import.json"),
        sidecar.clone(),
    );
    let binding = bindings
        .existing("texture.png")
        .expect("supported upgrade")
        .expect("recognized source");
    assert_eq!(binding.version, "4-current");
    assert_eq!(
        bindings.workspace.doc("as_source"),
        Some(&sidecar),
        "organization discovery rewrote a sidecar"
    );
    let mut newer = sidecar;
    let version = ara_asset::SidecarCommand::Version("5-future".into())
        .edit(newer.id.clone())
        .expect("version edit");
    newer
        .apply(version, &bindings.schema)
        .expect("newer sidecar");
    bindings.workspace.insert(
        bindings.workspace.storage_path("texture.png.import.json"),
        newer,
    );
    assert!(
        bindings
            .existing("texture.png")
            .expect_err("newer importer must not be downgraded")
            .to_string()
            .contains("update Ara")
    );
}

#[test]
fn build_profile_uses_its_worker_without_importer_discovery_and_is_undoable() {
    let scratch = scratch();
    write_project(scratch.path());
    let mut host = open(scratch.path());
    let session = begin(&mut host, None);
    let preset = ara_host::project_layout::default_build_preset("Game");
    let started = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"why":"Create a desktop build profile","commands":[{"cmd":"build_profile.create","args":{"name":"desktop","preset":preset}}]}),
    );
    complete(
        &mut host,
        &session,
        started["job"].as_str().expect("profile preparation job"),
    );
    let path = scratch.path().join("settings/builds/desktop.export.json");
    let saved =
        ara_json::parse(&fs::read_to_string(&path).expect("physically saved build profile"))
            .expect("canonical profile");
    assert_eq!(saved, preset);
    let undo = call(&mut host, "history.undo", json!({"session":session}));
    if let Some(job) = undo["job"].as_str() {
        complete(&mut host, &session, job);
    }
    host.flush();
    assert!(!path.exists());
}
