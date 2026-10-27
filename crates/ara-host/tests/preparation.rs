//! Subsystem preparation runs only after request validation, before any transaction.
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ara_diag::{Diagnostic, codes};
use ara_doc::file_state::PreparedInventory;
use ara_host::{Cx, Host, HostConfig, Method, Project, Service, params::ApplyParams};
use ara_proto::ConnId;
use ara_schema::SchemaDb;
use serde_json::{Value, json};

struct Pending {
    batch: ApplyParams,
    revision: u64,
    job: String,
}

struct PreparationSpy {
    calls: Arc<AtomicUsize>,
    pending: Option<Pending>,
}

impl Service for PreparationSpy {
    fn name(&self) -> &'static str {
        "preparation-spy"
    }
    fn methods(&self, _db: &mut SchemaDb) -> Vec<Method> {
        Vec::new()
    }
    fn call(
        &mut self,
        _cx: &mut Cx<'_>,
        _method: &str,
        _params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        Err(vec![Diagnostic::error(
            codes::UNKNOWN_METHOD,
            "the preparation spy exposes no RPC methods",
        )])
    }
    fn prepare_apply(
        &mut self,
        cx: &mut Cx<'_>,
        params: &ApplyParams,
    ) -> Option<Result<Value, Vec<Diagnostic>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(cx.session().expect("validated session").id, params.session);
        let related = params.commands.iter().any(|command| {
            command.cmd == "entity.create"
                && command
                    .args
                    .as_ref()
                    .and_then(|args| args.get("name"))
                    .and_then(Value::as_str)
                    == Some("Prepared entity")
        });
        if !related {
            return None;
        }
        let batch =
            serde_json::from_value(serde_json::to_value(params).expect("serializable batch"))
                .expect("copy validated batch");
        let job = cx.start_job("fixture.prepare");
        self.pending = Some(Pending {
            batch,
            revision: cx.store().revision(),
            job: job.clone(),
        });
        Some(Ok(json!({"ok":true,"job":job,"preparing":true})))
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let session = pending.batch.session.clone();
        let result = cx
            .resume_session(&session, |cx| {
                cx.apply_prepared(
                    pending.batch,
                    PreparedInventory::default(),
                    pending.revision,
                )
            })
            .and_then(std::convert::identity);
        cx.finish_job(&pending.job, result);
    }
}

fn scratch() -> PathBuf {
    let directory = std::env::var_os("ARA_TEST_ROOT")
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ara-preparation-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
    fs::create_dir_all(&directory).expect("fresh scratch project");
    directory
}

fn batch(session: &str, id: &str, name: &str) -> Value {
    json!({"session":session,"doc":"sc_main","why":format!("Create {name}"),"commands":[
        {"cmd":"entity.create","args":{"id":id,"name":name}}
    ]})
}

#[test]
fn preparation_validates_before_hook_defers_ops_and_falls_back_to_normal_apply() {
    let root = scratch();
    fs::write(
        root.join("ara.project.json"),
        r#"{"layoutVersion":3,"main":"scenes/main.scene"}"#,
    )
    .expect("marker");
    let scene = root.join(ara_doc::AUTHORING_ROOT).join("scenes/main.scene");
    fs::create_dir_all(scene.parent().expect("scene directory")).expect("authored directory");
    fs::write(
        scene,
        r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{}}"#,
    )
    .expect("scene");
    let mut host = Host::open(
        Project::open(&root).expect("project"),
        HostConfig {
            seed: Some(7391),
            debounce: Duration::from_secs(3600),
            watch_every: Duration::from_secs(3600),
            scan_every: Duration::from_secs(3600),
            ..HostConfig::default()
        },
    )
    .expect("host");
    let calls = Arc::new(AtomicUsize::new(0));
    host.register(Box::new(PreparationSpy {
        calls: Arc::clone(&calls),
        pending: None,
    }))
    .expect("register preparer");
    let begun = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"preparation-test","author":"agent:preparation"}),
        )
        .expect("session");
    let session = begun["session"]
        .as_str()
        .expect("session handle")
        .to_owned();
    host.call(
        ConnId::LOCAL,
        "subscribe",
        json!({"session":session,"topics":["ops"]}),
    )
    .expect("subscribe ops");
    host.drain_outbox();
    let revision = host.store().revision();
    let history = host.store().history().len();

    let invalid = host
        .call(
            ConnId::LOCAL,
            "cmd.apply",
            batch("missing-session", "e_prepared", "Prepared entity"),
        )
        .expect_err("invalid session");
    assert!(
        invalid
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == codes::SESSION_NOT_FOUND)
    );
    host.call(
        ConnId::LOCAL,
        "cmd.apply",
        json!({"session":session,"commands":"malformed"}),
    )
    .expect_err("malformed envelope");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "invalid requests must never reach a preparer"
    );

    let preparing = host
        .call(
            ConnId::LOCAL,
            "cmd.apply",
            batch(&session, "e_prepared", "Prepared entity"),
        )
        .expect("queued preparation");
    assert_eq!(preparing["preparing"], true);
    assert!(preparing["job"].is_string());
    assert!(preparing.get("ops").is_none());
    assert_eq!(host.store().revision(), revision);
    assert_eq!(host.store().history().len(), history);
    assert!(
        host.store()
            .workspace()
            .doc("sc_main")
            .expect("scene")
            .record("e_prepared")
            .is_none()
    );
    assert!(
        !host
            .drain_outbox()
            .iter()
            .any(|(_, notice)| notice["method"] == "ops")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Complete on a host tick, whose default context has no calling session.
    host.tick();
    let job = host
        .call(
            ConnId::LOCAL,
            "jobs.status",
            json!({"session":session,"job":preparing["job"]}),
        )
        .expect("job status");
    assert_eq!(job["status"], "done", "{job}");
    assert_eq!(job["session"], session);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "checked completion must bypass preparation"
    );
    assert_eq!(host.store().revision(), revision + 1);
    let transaction = host.store().history().last().expect("prepared transaction");
    assert_eq!(transaction.author.to_string(), "agent:preparation");
    assert_eq!(transaction.why.as_deref(), Some("Create Prepared entity"));
    assert!(
        host.store()
            .workspace()
            .doc("sc_main")
            .expect("scene")
            .record("e_prepared")
            .is_some()
    );

    let applied = host
        .call(
            ConnId::LOCAL,
            "cmd.apply",
            batch(&session, "e_normal", "Normal entity"),
        )
        .expect("normal fallback");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "unrelated batch was offered and declined"
    );
    assert_eq!(applied["ok"], true);
    assert!(applied["txn"].is_string());
    assert!(!applied["ops"].as_array().expect("normal ops").is_empty());
    assert_eq!(host.store().revision(), revision + 2);
    assert!(
        host.store()
            .workspace()
            .doc("sc_main")
            .expect("scene")
            .record("e_normal")
            .is_some()
    );
}
