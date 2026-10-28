//! Read-only validation jobs over one loaded-authoring revision.
#[path = "snapshot.rs"]
mod snapshot;
#[cfg(test)]
#[path = "tests.rs"]
#[allow(clippy::unwrap_used)]
mod tests;
#[path = "worker.rs"]
mod worker;
use ara_diag::{Diagnostic, codes};
use ara_host::{Cx, Method, Service, parse};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use snapshot::Preparing;
use std::{
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Parameters of the read-only loaded-document validation job.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Validate {
    /// Calling session with read scope.
    session: String,
}
enum Work {
    Pending,
    Preparing(Box<Preparing>),
    Running(worker::Running),
    Publishing(worker::Report),
    Retiring(JoinHandle<()>),
    Finished,
}
struct Job {
    id: String,
    revision: u64,
    generation: u64,
    work: Work,
    reported: Option<Instant>,
}
impl Job {
    fn phase(&self) -> &'static str {
        match &self.work {
            Work::Pending => "pending",
            Work::Preparing(_) => "preparing",
            Work::Running(_) => "checking",
            Work::Publishing(_) => "publishing",
            Work::Retiring(_) | Work::Finished => "stopping",
        }
    }
}
/// One active read-only job. Validation and large snapshot disposal run off-thread.
#[derive(Default)]
pub(crate) struct ValidationService {
    active: Option<Job>,
}

fn failure(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(crate::codes::ACTION, message)
}
impl Service for ValidationService {
    fn name(&self) -> &'static str {
        "editor-validation"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![Method::new::<Validate>(db,"editor.validate","Validate a revision-stamped snapshot of loaded authoring documents; schema, references, hierarchy and prefab issues appear in Problems.").read_only()]
    }
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        if method != "editor.validate" {
            return Err(vec![Diagnostic::error(
                codes::UNKNOWN_METHOD,
                "Unknown validation method",
            )]);
        }
        let params: Validate = parse(params)?;
        let _ = &params.session;
        if let Some(job) = &self.active {
            if cx.job_cancelled(&job.id) || matches!(job.work, Work::Retiring(_) | Work::Finished) {
                return Err(vec![failure(
                    "The previous validation job is stopping. Try again when its worker has finished.",
                )]);
            }
            return Ok(
                json!({"job":job.id,"revision":job.revision,"scope":"loadedAuthoringDocuments","running":true,"phase":job.phase()}),
            );
        }
        let id = cx.start_job("Validate loaded authoring documents");
        let revision = cx.store().revision();
        self.active = Some(Job {
            id: id.clone(),
            revision,
            generation: cx.store().workspace().generation(),
            work: Work::Pending,
            reported: None,
        });
        Ok(
            json!({"job":id,"revision":revision,"scope":"loadedAuthoringDocuments","preparing":true,"phase":"pending"}),
        )
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        let Some(mut job) = self.active.take() else {
            return;
        };
        let cancelled = cx.job_cancelled(&job.id);
        let stale = job.revision != cx.store().revision()
            || job.generation != cx.store().workspace().generation();
        let work = std::mem::replace(&mut job.work, Work::Pending);
        match work {
            Work::Pending if cancelled => return,
            Work::Pending if stale => {
                finish_error(
                    cx,
                    &job,
                    "The project changed before validation could prepare its snapshot. Run validation again.",
                );
                return;
            }
            Work::Pending => job.work = Work::Preparing(Box::new(Preparing::new(cx.store()))),
            Work::Preparing(preparing) if cancelled || stale => {
                if stale && !cancelled {
                    finish_error(
                        cx,
                        &job,
                        "The project changed while its validation snapshot was being prepared. Run validation again.",
                    );
                }
                job.work = retire(preparing);
            }
            Work::Preparing(mut preparing) => match preparing.advance(cx.store()) {
                Ok(true) => match worker::Running::start(preparing.finish(), job.revision) {
                    Ok(running) => {
                        cx.job_progress(&job.id, 0.25, "Checking loaded authoring documents");
                        job.reported = Some(Instant::now());
                        job.work = Work::Running(running);
                    }
                    Err(error) => {
                        finish_error(cx, &job, &error.to_string());
                        return;
                    }
                },
                Ok(false) => {
                    report(
                        cx,
                        &mut job,
                        preparing.fraction() * 0.25,
                        "Preparing loaded authoring snapshot",
                    );
                    job.work = Work::Preparing(preparing);
                }
                Err(error) => {
                    finish_error(cx, &job, &error);
                    job.work = retire(preparing);
                }
            },
            Work::Running(running) => {
                if cancelled {
                    running.cancel();
                }
                if running.finished() {
                    match running.complete() {
                        Ok(report) if !cancelled && !report.cancelled => {
                            cx.job_progress(
                                &job.id,
                                0.9,
                                "Publishing validation diagnostics to Problems",
                            );
                            job.reported = Some(Instant::now());
                            job.work = Work::Publishing(report);
                        }
                        Ok(report) => job.work = retire(report),
                        Err(error) => {
                            finish_error(cx, &job, &error);
                            return;
                        }
                    }
                } else {
                    if !cancelled {
                        report(
                            cx,
                            &mut job,
                            0.25 + running.fraction() * 0.65,
                            "Checking loaded authoring documents",
                        );
                    }
                    job.work = Work::Running(running);
                }
            }
            Work::Publishing(mut result) => {
                if cancelled {
                    job.work = retire(result);
                } else {
                    let started = Instant::now();
                    for _ in 0..64 {
                        if started.elapsed() >= Duration::from_millis(2) {
                            break;
                        }
                        let Some(diagnostic) = result.diagnostics.pop_front() else {
                            break;
                        };
                        cx.log(diagnostic);
                        result.published += 1;
                    }
                    if result.diagnostics.is_empty() {
                        cx.finish_job(&job.id,Ok(json!({"scope":"loadedAuthoringDocuments","revision":job.revision,
                            "currentRevision":cx.store().revision(),"stale":stale,"checked":result.checked,"records":result.records,
                            "errors":result.errors,"warnings":result.warnings,"diagnosticsPublished":result.published,
                            "note":"Loaded in-memory schema/reference/structure/prefab checks. Existing parse failures remain in Problems; this job does not scan or rewrite disk files."})));
                        return;
                    }
                    report(
                        cx,
                        &mut job,
                        0.95,
                        "Publishing validation diagnostics to Problems",
                    );
                    job.work = Work::Publishing(result);
                }
            }
            Work::Retiring(thread) => {
                if thread.is_finished() {
                    let _ = thread.join();
                    return;
                }
                job.work = Work::Retiring(thread);
            }
            Work::Finished => return,
        }
        self.active = Some(job);
    }
}
fn finish_error(cx: &mut Cx<'_>, job: &Job, message: &str) {
    let diagnostic = failure(message);
    cx.log(diagnostic.clone());
    cx.finish_job(&job.id, Err(vec![diagnostic]));
}
fn report(cx: &mut Cx<'_>, job: &mut Job, fraction: f32, message: &str) {
    let now = Instant::now();
    if job
        .reported
        .is_none_or(|at| now.duration_since(at) >= Duration::from_millis(100))
    {
        cx.job_progress(&job.id, fraction, message);
        job.reported = Some(now);
    }
}
fn retire(value: impl Send + 'static) -> Work {
    match std::thread::Builder::new()
        .name("editor-validation-dispose".into())
        .spawn(move || drop(value))
    {
        Ok(thread) => Work::Retiring(thread),
        Err(_) => Work::Finished,
    }
}
