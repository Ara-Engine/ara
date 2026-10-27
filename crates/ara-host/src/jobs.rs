//! The jobs table: long-running work with progress (import, bake, build, generate).
//!
//! Registry jobs run inside one transaction when started. Services run their own
//! jobs across host ticks: [`Cx::start_job`](crate::Cx::start_job), then
//! `job_progress` and `finish_job`, checking `job_cancelled`. Every progress step is
//! broadcast on the `jobs` topic.

use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use serde::Serialize;
use serde_json::Value;

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    /// Still working.
    Running,
    /// Finished; `result` is set.
    Done,
    /// Failed; `diagnostics` say why.
    Failed,
    /// Cancelled on request.
    Cancelled,
}

/// One job.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    /// Job ID (`jb_...`).
    pub job: String,
    /// What runs (`asset.import`).
    pub name: String,
    /// Session that started it.
    pub session: Option<String>,
    /// Status.
    pub status: JobStatus,
    /// Whether cancellation can still prevent this job's mutation.
    pub cancellable: bool,
    /// Progress in `[0, 1]`.
    pub progress: f32,
    /// Latest progress message.
    pub message: String,
    /// Result when done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Why it failed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

/// Every job of this host run.
#[derive(Debug, Default)]
pub struct Jobs {
    by_id: BTreeMap<String, JobRecord>,
}

impl Jobs {
    /// Adds a running job.
    pub fn start(&mut self, id: String, name: &str, session: Option<String>) -> &JobRecord {
        self.by_id.entry(id.clone()).or_insert(JobRecord {
            job: id,
            name: name.to_owned(),
            session,
            status: JobStatus::Running,
            cancellable: true,
            progress: 0.0,
            message: String::new(),
            result: None,
            diagnostics: Vec::new(),
        })
    }

    /// Records progress on a running job; returns it if it is still running.
    pub fn progress(&mut self, id: &str, fraction: f32, message: &str) -> Option<&JobRecord> {
        let job = self
            .by_id
            .get_mut(id)
            .filter(|j| j.status == JobStatus::Running)?;
        job.progress = fraction.clamp(0.0, 1.0);
        job.message = message.to_owned();
        Some(job)
    }

    /// Finishes a running job.
    pub fn finish(
        &mut self,
        id: &str,
        result: Result<Value, Vec<Diagnostic>>,
    ) -> Option<&JobRecord> {
        let job = self
            .by_id
            .get_mut(id)
            .filter(|j| j.status == JobStatus::Running)?;
        job.cancellable = false;
        match result {
            Ok(v) => {
                job.status = JobStatus::Done;
                job.progress = 1.0;
                job.result = Some(v);
            }
            Err(d) => {
                job.status = JobStatus::Failed;
                job.diagnostics = d;
            }
        }
        Some(job)
    }

    /// Marks the irreversible commit boundary; cancellation is refused afterward.
    pub fn committing(&mut self, id: &str) -> Option<&JobRecord> {
        let job = self
            .by_id
            .get_mut(id)
            .filter(|job| job.status == JobStatus::Running && job.cancellable)?;
        job.cancellable = false;
        job.message = "Saving changes".into();
        Some(job)
    }

    /// Marks a cancellable running job cancelled. Returns false after commit starts.
    pub fn cancel(&mut self, id: &str) -> bool {
        match self.by_id.get_mut(id) {
            Some(j) if j.status == JobStatus::Running && j.cancellable => {
                j.status = JobStatus::Cancelled;
                j.cancellable = false;
                true
            }
            _ => false,
        }
    }

    /// One job.
    pub fn get(&self, id: &str) -> Option<&JobRecord> {
        self.by_id.get(id)
    }

    /// Every job, by ID.
    pub fn all(&self) -> impl Iterator<Item = &JobRecord> {
        self.by_id.values()
    }
}
