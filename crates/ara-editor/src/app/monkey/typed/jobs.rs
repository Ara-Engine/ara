//! Only exact jobs returned by this run's typed calls are owned or cancelled.
use super::{Action, Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Jobs {
    baseline_collected: bool,
    baseline: BTreeSet<String>,
    owned: BTreeMap<String, Job>,
}
struct Job {
    command: String,
    status: Value,
    cancel_sent: bool,
}
impl Jobs {
    pub fn baseline(&mut self, response: &Value) -> Result<(), String> {
        if self.baseline_collected {
            return Ok(());
        }
        let jobs = response["jobs"]
            .as_array()
            .ok_or("jobs.list returned no inventory")?;
        self.baseline = jobs
            .iter()
            .filter_map(|j| j["job"].as_str().map(str::to_owned))
            .collect();
        self.baseline_collected = true;
        Ok(())
    }
    pub fn returned(&mut self, command: &str, response: &Value) {
        let Some(id) = response["job"].as_str() else {
            return;
        };
        if self.baseline.contains(id) {
            return;
        }
        self.owned.entry(id.into()).or_insert_with(|| Job {
            command: command.into(),
            status: json!({"job":id,"status":"running"}),
            cancel_sent: false,
        });
    }
    pub fn owns(&self, id: &str) -> bool {
        self.owned.contains_key(id)
    }
    pub fn observe(&mut self, id: &str, response: &Value) -> Option<String> {
        let job = self.owned.get_mut(id)?;
        // Cancellation responses can be acknowledgements, not final status.
        if response["status"].is_string() {
            job.status = json!({"status":response["status"],"cancellable":response["cancellable"],
                "progress":response["progress"],"message":response["message"],"diagnostics":response["diagnostics"]});
        }
        Some(job.command.clone())
    }
    pub fn next(&mut self, cancel: bool) -> Option<Action> {
        let (id, job) = self.owned.iter_mut().find(|(_, j)| !terminal(&j.status))?;
        let cancel = cancel && !job.cancel_sent && job.status["cancellable"] == true;
        if cancel {
            job.cancel_sent = true;
        }
        Some(Action::Protocol {
            method: if cancel { "jobs.cancel" } else { "jobs.status" }.into(),
            params: json!({"job":id}),
            raw: false,
        })
    }
    pub fn drained(&self) -> bool {
        self.owned.values().all(|job| terminal(&job.status))
    }
    pub fn report(&self) -> Value {
        json!({"baselineJobs":self.baseline,"ownedJobs":self.owned.iter().map(|(id,j)| {
            json!({"job":id,"command":j.command,"status":j.status["status"],"cancellable":j.status["cancellable"],"progress":j.status["progress"],"message":j.status["message"],"diagnostics":j.status["diagnostics"],"cancelRequested":j.cancel_sent})
        }).collect::<Vec<_>>(),"drained":self.drained()})
    }
}
fn terminal(value: &Value) -> bool {
    matches!(
        value["status"].as_str(),
        Some("done" | "failed" | "cancelled")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn drain_only_cancels_returned_owned_jobs_after_authority_marks_them_cancellable() {
        let mut jobs = Jobs::default();
        jobs.baseline(
            &json!({"jobs":[{"job":"jb_baseline","status":"running","cancellable":true}]}),
        )
        .unwrap();
        jobs.returned("folder.create", &json!({"job":"jb_baseline"}));
        assert!(jobs.drained());
        jobs.returned("folder.create", &json!({"job":"jb_owned"}));
        let next = |jobs: &mut Jobs| serde_json::to_value(jobs.next(true).unwrap()).unwrap();
        assert_eq!(
            next(&mut jobs)["method"],
            "jobs.status",
            "Unknown cancellability requires a status read"
        );
        jobs.observe(
            "jb_owned",
            &json!({"status":"running","cancellable":false,"progress":0.99}),
        );
        assert_eq!(
            next(&mut jobs)["method"],
            "jobs.status",
            "Committing must finish without speculative cancellation"
        );
        jobs.observe(
            "jb_owned",
            &json!({"status":"running","cancellable":true,"progress":0.2}),
        );
        let cancel = next(&mut jobs);
        assert_eq!(cancel["method"], "jobs.cancel");
        assert_eq!(cancel["params"]["job"], "jb_owned");
        jobs.observe("jb_owned", &json!({"cancelled":true}));
        assert!(
            !jobs.drained(),
            "An acknowledgement is not an observed terminal job"
        );
        assert_eq!(next(&mut jobs)["method"], "jobs.status");
        jobs.observe(
            "jb_owned",
            &json!({"status":"cancelled","cancellable":false}),
        );
        assert!(jobs.drained());
        assert!(jobs.next(true).is_none());
        assert!(!jobs.owns("jb_baseline"));
    }
}
