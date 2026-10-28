//! Controlled compiler envelopes through the real ScriptService poll and Host log/job APIs.
//! These transport/publication tests do not invoke Roslyn or claim compilation coverage.
#![allow(clippy::unwrap_used)]
use super::super::worker;
use super::*;
use ara_diag::{Diagnostic, Subject};
use ara_proto::Endpoint;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Request {
    session: String,
    response: Result<Value, Vec<Diagnostic>>,
}
struct Probe {
    script: ScriptService,
    input: Receiver<Request>,
    output: mpsc::Sender<String>,
}
impl Service for Probe {
    fn name(&self) -> &'static str {
        "script-ingress-proof"
    }
    fn methods(&self, _: &mut SchemaDb) -> Vec<Method> {
        Vec::new()
    }
    fn call(&mut self, _: &mut Cx<'_>, _: &str, _: Value) -> Result<Value, Vec<Diagnostic>> {
        Err(vec![crate::invalid("No probe RPC methods")])
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        if let Ok(request) = self.input.try_recv() {
            let job = cx
                .resume_session(&request.session, |cx| {
                    let job = cx.start_job("Controlled helper response ingress");
                    let (send, result) = mpsc::channel();
                    send.send(request.response).unwrap();
                    self.script.jobs.insert(
                        job.clone(),
                        Compile {
                            session: request.session.clone(),
                            path: String::new(),
                            doc: None,
                            attach: Vec::new(),
                            result,
                        },
                    );
                    job
                })
                .unwrap();
            self.script.poll(cx);
            assert!(
                self.script.jobs.is_empty(),
                "ready response must retire in the actual poll"
            );
            self.output.send(job).unwrap();
        }
    }
}
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("compiler-diagnostic-ingress")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path, value) in [
            (
                root.join("ara.project.json"),
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Compiler ingress","main":"levels/main.scene"}),
            ),
            (
                root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
                json!({"$schema":"ara://schema/scene/1","id":"sc_compileingress","records":{}}),
            ),
        ] {
            std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0
                .starts_with(std::path::absolute(crate::test_target()).unwrap())
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn diagnostics(error: bool) -> Vec<Diagnostic> {
    let row = |phase: &str, attempt: u64, severity: ara_diag::Severity, message: &str| {
        Diagnostic::new(severity,if phase=="diagnostic" {if severity==ara_diag::Severity::Error {ara_script::codes::COMPILE_ERROR}else{ara_script::codes::COMPILE_WARNING}}else{ara_script::codes::COMPILE_RESULT},message)
            .subject(Subject::module("Ingress.Gameplay"))
            .subject(Subject::system(format!("csharp.compile/cp_ingress/{attempt}")))
            .data(json!({"module":"Ingress.Gameplay","ingressFixture":true,"compile":{"schema":1,"producer":"project","instance":"cp_ingress","binding":4,"attempt":attempt,"profile":"gameplay","phase":phase,"channel":"compiler"}}))
    };
    let mut rows = vec![
        row(
            "bound",
            0,
            ara_diag::Severity::Info,
            "Ingress producer bound",
        ),
        row(
            "started",
            7,
            ara_diag::Severity::Info,
            "Ingress attempt started",
        ),
        row(
            "diagnostic",
            7,
            ara_diag::Severity::Warning,
            "Ingress compiler warning",
        ),
    ];
    if error {
        rows.push(row(
            "diagnostic",
            7,
            ara_diag::Severity::Error,
            "Ingress compiler error",
        ));
    }
    let mut completed = row(
        "completed",
        7,
        ara_diag::Severity::Info,
        "Ingress attempt completed",
    );
    let metadata = &mut completed.data.as_mut().unwrap()["compile"];
    metadata["result"] = json!(if error { "errors" } else { "success" });
    metadata["validation"] = json!("current-module-inputs");
    metadata["errorCount"] = json!(usize::from(error));
    metadata["total"] = json!(1 + usize::from(error));
    metadata["emitted"] = json!(1 + usize::from(error));
    metadata["truncated"] = json!(false);
    metadata["coverage"] = json!({"errors":"module","warnings":"partial"});
    rows.push(completed);
    rows
}
fn helper(error: bool) -> (Value, Value) {
    let rows = diagnostics(error);
    let expected = json!(rows);
    let inventory = ara_script::project::Inventory {
        diagnostics: rows,
        ..Default::default()
    };
    let response = worker::response(worker::compiled(inventory));
    // Exactly the canonical stdout JSON boundary, rather than passing an in-memory
    // typed Vec around the production helper decoder.
    let response: Value = serde_json::from_str(&ara_json::to_canonical_string(&response)).unwrap();
    assert_eq!(response["diagnostics"], expected);
    assert_eq!(response["ok"], !error);
    (response, expected)
}
fn run(response: Result<Value, Vec<Diagnostic>>) -> (Value, Vec<Value>) {
    let project = Project::new();
    let mut editor =
        crate::Editor::open_with(&project.0, Some(project.0.join(".ara/test-settings.json")))
            .unwrap();
    let before = editor.host.store().revision();
    let (send, input) = mpsc::channel();
    let (output, done) = mpsc::channel();
    editor
        .host
        .register(Box::new(Probe {
            script: ScriptService::default(),
            input,
            output,
        }))
        .unwrap();
    let session = Endpoint::call(
        &mut editor.host,
        "session.begin",
        json!({"client":"compiler-ingress-test","author":"human"}),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    send.send(Request {
        session: session.clone(),
        response,
    })
    .unwrap();
    editor.tick();
    let job = done.try_recv().unwrap();
    let status = Endpoint::call(
        &mut editor.host,
        "jobs.status",
        json!({"session":session,"job":job}),
    )
    .unwrap();
    let logs = Endpoint::call(
        &mut editor.host,
        "logs",
        json!({"session":session,"severity":"info","limit":1000}),
    )
    .unwrap();
    let mut rows: Vec<_> = logs["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| {
            entry["diagnostic"]["data"]["ingressFixture"] == true
                || entry["diagnostic"]["message"] == "Compile returned no reflected types"
                || entry["diagnostic"]["message"] == "Ingress transport failed"
        })
        .cloned()
        .collect();
    rows.sort_by_key(|entry| entry["seq"].as_u64());
    assert_eq!(
        editor.host.store().revision(),
        before,
        "diagnostic publication and an empty schema must not author content"
    );
    assert!(
        rows.iter().all(|entry| entry["count"] == 1),
        "no compiler/publication error may be logged twice: {rows:?}"
    );
    (status, rows)
}

#[test]
fn ordered_info_and_warning_diagnostics_survive_successful_helper_and_real_job_poll() {
    let (response, expected) = helper(false);
    let (status, rows) = run(Ok(response));
    assert_eq!(status["status"], "done");
    assert_eq!(status["result"]["diagnostics"], expected);
    assert_eq!(
        json!(
            rows.iter()
                .map(|entry| &entry["diagnostic"])
                .collect::<Vec<_>>()
        ),
        expected
    );
}
#[test]
fn failed_helper_preserves_lifecycle_warnings_and_true_errors_without_duplicate_logs() {
    let (response, expected) = helper(true);
    let (status, rows) = run(Ok(response));
    assert_eq!(status["status"], "failed");
    assert_eq!(status["diagnostics"], expected);
    assert_eq!(
        json!(
            rows.iter()
                .map(|entry| &entry["diagnostic"])
                .collect::<Vec<_>>()
        ),
        expected
    );
}
#[test]
fn true_error_severity_rejects_even_an_ok_envelope_without_publishing_schema() {
    let (mut response, expected) = helper(true);
    response["ok"] = json!(true);
    response["types"] = json!({});
    let (status, rows) = run(Ok(response));
    assert_eq!(status["status"], "failed");
    assert_eq!(status["diagnostics"], expected);
    assert_eq!(rows.len(), expected.as_array().unwrap().len());
}
#[test]
fn later_publication_and_transport_failures_are_each_logged_once() {
    let (mut response, expected) = helper(false);
    response["types"] = Value::Null;
    let (status, rows) = run(Ok(response));
    assert_eq!(status["status"], "failed");
    assert_eq!(rows.len(), expected.as_array().unwrap().len() + 1);
    assert_eq!(
        rows.last().unwrap()["diagnostic"]["message"],
        "Compile returned no reflected types"
    );
    let diagnostic = Diagnostic::error(crate::codes::ACTION, "Ingress transport failed");
    let (status, rows) = run(Err(vec![diagnostic]));
    assert_eq!(status["status"], "failed");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["count"], 1);
}
