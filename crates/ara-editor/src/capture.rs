//! Captures run as jobs at a render boundary; protocol IO never owns the GPU.
use ara_diag::Diagnostic;
use ara_host::{Cx, Method, Service};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::mpsc::{Receiver, Sender};

/// Parameters of an editor or viewport screenshot job.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaptureParams {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// `viewport`, `editor`, or `layout`.
    pub target: Option<String>,
    /// Relative output PNG path under the project (default `.ara/captures/<job>.png`).
    pub path: Option<String>,
    /// Numbered viewport marks and legend.
    pub marks: Option<u32>,
}
pub(crate) struct CaptureTask {
    pub job: String,
    pub params: CaptureParams,
}
pub(crate) struct CaptureCompletion {
    pub job: String,
    pub result: Result<Value, Vec<Diagnostic>>,
}
pub(crate) struct CaptureService {
    tasks: Sender<CaptureTask>,
    results: Receiver<CaptureCompletion>,
}
impl CaptureService {
    pub fn new(tasks: Sender<CaptureTask>, results: Receiver<CaptureCompletion>) -> Self {
        Self { tasks, results }
    }
}
impl Service for CaptureService {
    fn name(&self) -> &'static str {
        "capture"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![Method::new::<CaptureParams>(db,"capture","Capture the viewport with entity marks, editor UI or layout report; returns a job.").tool("ara_capture").read_only()]
    }
    fn call(&mut self, cx: &mut Cx<'_>, _: &str, params: Value) -> Result<Value, Vec<Diagnostic>> {
        let params: CaptureParams = ara_host::parse(params)?;
        if !matches!(
            params.target.as_deref().unwrap_or("viewport"),
            "viewport" | "editor" | "layout"
        ) {
            return Err(vec![crate::invalid(
                "capture target must be viewport, editor or layout",
            )]);
        }
        let job = cx.start_job("capture");
        cx.job_progress(&job, 0., "Waiting for render boundary");
        self.tasks
            .send(CaptureTask {
                job: job.clone(),
                params,
            })
            .map_err(|_| vec![crate::invalid("editor renderer has closed")])?;
        Ok(json!({"job":job,"status":"running"}))
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        while let Ok(completion) = self.results.try_recv() {
            if !cx.job_cancelled(&completion.job) {
                cx.finish_job(&completion.job, completion.result);
            }
        }
    }
}
