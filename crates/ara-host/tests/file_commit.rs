//! A prepared job reports physical completion separately from its logical transaction.
use ara_cmd::{Command, EditCtx};
use ara_diag::{Diagnostic, codes};
use ara_doc::{
    Edit,
    file_state::{FileState, PreparedInventory},
};
use ara_host::{
    Cx, FileCommitReceipt, Host, HostConfig, Method, Project, Service,
    params::{ApplyParams, JobParams},
};
use ara_proto::{ConnId, Scope};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// No filesystem work occurs in this command; the missing snapshot fails on the file worker.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(deny_unknown_fields)]
struct MissingBlobParams {}
struct MissingBlob;
impl Command for MissingBlob {
    type Params = MissingBlobParams;
    type Output = Value;
    const NAME: &'static str = "fixture.missingBlob";
    const SUMMARY: &'static str = "Stage a valid token whose immutable bytes are unavailable.";
    fn examples() -> Vec<Value> {
        vec![json!({})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, _: Self::Params) -> Result<Value, Diagnostic> {
        let doc = cx.read().doc().id.clone();
        let path = cx.read().workspace().storage_path("missing.bin");
        cx.apply_edit(Edit::File {
            doc,
            path,
            base: None,
            state: Some(FileState::bytes(b"never stored in file history")),
        })?;
        Ok(Value::Null)
    }
}

#[derive(Default)]
struct CommitService {
    pending: BTreeMap<String, (FileCommitReceipt, Value)>,
}
impl Service for CommitService {
    fn name(&self) -> &'static str {
        "commit-fixture"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<ApplyParams>(db, "fixture.commit", "Commit a prepared fixture batch.")
                .scope(Scope::EditScene),
            Method::new::<JobParams>(db, "fixture.poll", "Poll fixture physical completion.")
                .scope(Scope::EditScene),
        ]
    }
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        if method == "fixture.commit" {
            let params: ApplyParams = ara_host::parse(params)?;
            let mut inventory = PreparedInventory::default();
            if params
                .commands
                .iter()
                .any(|command| command.cmd == MissingBlob::NAME)
            {
                inventory
                    .entries
                    .insert(cx.store().workspace().storage_path("missing.bin"), None);
            }
            let job = cx.start_job("fixture.fileCommit");
            match cx.apply_prepared_job(&job, params, inventory, cx.store().revision()) {
                Ok((value, receipt)) => {
                    self.pending.insert(job.clone(), (receipt, value));
                    Ok(json!({"job":job}))
                }
                Err(errors) => {
                    cx.finish_job(&job, Err(errors.clone()));
                    Err(errors)
                }
            }
        } else {
            let params: JobParams = ara_host::parse(params)?;
            let ready = self
                .pending
                .get(&params.job)
                .and_then(|(receipt, _)| cx.poll_file_commit(*receipt));
            if let Some(result) = ready {
                let (_, value) = self.pending.remove(&params.job).expect("pending receipt");
                cx.finish_job(&params.job, result.map(|()| value));
                Ok(json!({"complete":true}))
            } else {
                Ok(json!({"complete":false}))
            }
        }
    }
}

fn fixture(name: &str) -> (PathBuf, Host, String) {
    let root = std::env::var_os("ARA_TEST_ROOT")
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ara-file-commit-tests")
        .join(format!(
            "{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
    fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("scenes")).expect("scratch");
    fs::write(
        root.join("ara.project.json"),
        r#"{"layoutVersion":3,"main":"scenes/main.scene"}"#,
    )
    .expect("marker");
    fs::write(
        root.join(ara_doc::AUTHORING_ROOT).join("scenes/main.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_main","records":{}}"#,
    )
    .expect("scene");
    let mut project = Project::open(&root).expect("project");
    project.registry.command::<MissingBlob>(&mut project.db);
    let mut host = Host::open(
        project,
        HostConfig {
            seed: Some(9307),
            debounce: Duration::from_secs(3600),
            watch_every: Duration::from_secs(3600),
            scan_every: Duration::from_secs(3600),
            ..HostConfig::default()
        },
    )
    .expect("host");
    host.register(Box::new(CommitService::default()))
        .expect("service");
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"file-commit-test","author":"agent:receipt"}),
        )
        .expect("session")["session"]
        .as_str()
        .expect("handle")
        .to_owned();
    (root, host, session)
}

fn complete(host: &mut Host, session: &str, job: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let polled = host
            .call(
                ConnId::LOCAL,
                "fixture.poll",
                json!({"session":session,"job":job}),
            )
            .expect("poll");
        if polled["complete"] == true {
            return host
                .call(
                    ConnId::LOCAL,
                    "jobs.status",
                    json!({"session":session,"job":job}),
                )
                .expect("status");
        }
        assert!(Instant::now() < deadline, "physical completion timed out");
        std::thread::yield_now();
    }
}

#[test]
fn commit_refuses_cancellation_and_acknowledges_canonical_bytes_and_flush_journal() {
    let (root, mut host, session) = fixture("canonical");
    let started=host.call(ConnId::LOCAL,"fixture.commit",json!({"session":session,"doc":"sc_main","why":"Persist the prepared entity","commands":[{"cmd":"entity.create","args":{"id":"e_saved","name":"Saved"}}]})).expect("logical commit");
    let job = &started["job"];
    let status = host
        .call(
            ConnId::LOCAL,
            "jobs.status",
            json!({"session":session,"job":job}),
        )
        .expect("committing job");
    assert_eq!(status["status"], "running");
    assert_eq!(status["cancellable"], false);
    let cancelled = host
        .call(
            ConnId::LOCAL,
            "jobs.cancel",
            json!({"session":session,"job":job}),
        )
        .expect("cancel request");
    assert_eq!(cancelled["cancelled"], false);
    assert_eq!(complete(&mut host, &session, job)["status"], "done");
    let text = fs::read_to_string(root.join(ara_doc::AUTHORING_ROOT).join("scenes/main.scene"))
        .expect("saved scene");
    let scene: Value = serde_json::from_str(&text).expect("canonical JSON");
    assert_eq!(scene["records"]["e_saved"]["name"], "Saved");
    let journal = fs::read_to_string(root.join(".ara/journal")).expect("durable journal");
    assert!(
        journal
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|entry| entry["t"] == "flush" && entry["doc"] == "sc_main"),
        "the receipt includes the Saved reply's journal append"
    );
}

#[test]
fn failed_materialization_fails_the_receipt_instead_of_reporting_done() {
    let (root, mut host, session) = fixture("missing-blob");
    let started=host.call(ConnId::LOCAL,"fixture.commit",json!({"session":session,"doc":"ara.project.json","why":"Exercise a physical write failure","commands":[{"cmd":MissingBlob::NAME,"args":{}}]})).expect("logical file transaction");
    let status = complete(&mut host, &session, &started["job"]);
    assert_eq!(status["status"], "failed", "{status}");
    assert!(
        status["diagnostics"]
            .as_array()
            .expect("failure diagnostics")
            .iter()
            .any(|error| error["code"] == codes::IO.to_string())
    );
    assert!(
        !root
            .join(ara_doc::AUTHORING_ROOT)
            .join("missing.bin")
            .exists()
    );
}
