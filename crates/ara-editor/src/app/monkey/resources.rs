//! Final job ownership comes from authority inventory, including shell-started work.
use super::Action;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Resources {
    session: Option<String>,
    baseline: BTreeSet<String>,
    owned: BTreeMap<String, Value>,
    cancelled: BTreeSet<String>,
    final_inventory: bool,
    errors: BTreeSet<String>,
    shell: Value,
    changed: bool,
    inventory_reads: u64,
    flush: Option<ara_host::FileCommitReceipt>,
    host_unsaved: Option<bool>,
    flush_requests: u64,
}
impl Resources {
    pub fn initialized(&self) -> bool {
        self.session.is_some()
    }
    pub fn baseline(&mut self, session: &str, response: &Value) -> Result<(), String> {
        let jobs = records(response)?;
        for job in jobs {
            let id = job["job"]
                .as_str()
                .ok_or("Startup job inventory contains no job ID")?;
            self.baseline.insert(id.into());
        }
        self.session = Some(session.into());
        self.changed = true;
        self.inventory_reads += 1;
        Ok(())
    }
    pub fn observe(&mut self, response: &Value) -> Result<(), String> {
        let session = self
            .session
            .as_deref()
            .ok_or("No startup job baseline was captured")?;
        let jobs = records(response)?;
        let mut present = BTreeSet::new();
        for job in jobs {
            let id = job["job"]
                .as_str()
                .ok_or("Final job inventory contains no job ID")?;
            if self.baseline.contains(id) || job["session"].as_str() != Some(session) {
                continue;
            }
            present.insert(id.to_owned());
            let status = job["status"]
                .as_str()
                .ok_or("Owned job has no authoritative status")?;
            if !matches!(status, "running" | "done" | "failed" | "cancelled") {
                self.changed |= self
                    .errors
                    .insert(format!("Unknown owned job status: {id} {status}"));
            }
            let record = json!({"job":id,"name":job["name"],"session":job["session"],
                "status":status,"cancellable":job["cancellable"],"progress":job["progress"],"message":job["message"]});
            if self.owned.get(id) != Some(&record) {
                self.owned.insert(id.into(), record);
                self.changed = true;
            }
        }
        for id in self.owned.keys().filter(|id| !present.contains(*id)) {
            self.changed |= self.errors.insert(format!(
                "Owned job vanished from the authority inventory: {id}"
            ));
        }
        self.changed |= !self.final_inventory;
        self.final_inventory = true;
        self.inventory_reads += 1;
        Ok(())
    }
    pub fn shell(&mut self, value: Value) {
        self.changed |= self.shell != value;
        self.shell = value;
    }
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
    /// Save existing command ops through the authority's nonblocking file queue.
    /// This creates no content transaction and never writes a document directly.
    pub fn settle_files(&mut self, editor: &mut super::Editor) {
        if self.flush.is_none() && editor.host.unsaved() && self.errors.is_empty() {
            self.flush = Some(editor.host.queue_flush());
            self.flush_requests += 1;
            self.changed = true;
        }
        if let Some(receipt) = self.flush
            && let Some(result) = editor.host.poll_flush(receipt)
        {
            self.flush = None;
            if let Err(errors) = result {
                for error in errors {
                    self.errors.insert(error.to_string());
                }
            }
            self.changed = true;
        }
        let unsaved = editor.host.unsaved();
        self.changed |= self.host_unsaved != Some(unsaved);
        self.host_unsaved = Some(unsaved);
    }
    pub fn next(&mut self) -> Option<Action> {
        if let Some((id, job)) = self.owned.iter().find(|(_, j)| !terminal(j)) {
            let cancel = job["cancellable"] == true && self.cancelled.insert(id.clone());
            self.changed |= cancel;
            return Some(Action::Protocol {
                method: if cancel { "jobs.cancel" } else { "jobs.status" }.into(),
                params: json!({"job":id}),
                raw: false,
            });
        }
        // Shell build/run completion is driven by its ordinary frame polling.
        // This keeps the event loop alive without a blocking wait or inferred edit.
        (!self.drained()).then(|| Action::Protocol {
            method: "jobs.list".into(),
            params: json!({}),
            raw: false,
        })
    }
    pub fn drained(&self) -> bool {
        self.initialized()
            && self.final_inventory
            && self.errors.is_empty()
            && self.shell["busy"] == false
            && self.owned.values().all(terminal)
            && self.host_unsaved == Some(false)
            && self.flush.is_none()
    }
    pub fn report(&self) -> Value {
        json!({"baselineCollected":self.initialized(),"baselineJobs":self.baseline,"session":self.session,"inventoryReads":self.inventory_reads,
            "finalInventoryCollected":self.final_inventory,"ownedJobs":self.owned.values().collect::<Vec<_>>(),
            "cancelRequests":self.cancelled,"shell":self.shell,"errors":self.errors,"drained":self.drained(),
            "hostUnsaved":self.host_unsaved,"fileFlushPending":self.flush.is_some(),"fileFlushRequests":self.flush_requests,"backendWorkerRetirementVerified":null,
            "limits":["Ownership includes only new jobs with this editor's human session; baseline and other-session jobs are never cancelled.",
                "Terminal job status confirms the authority's outcome, not physical retirement of every backend worker or child. Process exit/crash evidence does not establish that separately.",
                "Build/Run workers have no generic cancellation API; the runner waits through normal polling up to its drain limit."]})
    }
}
fn records(response: &Value) -> Result<&[Value], String> {
    response["jobs"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "jobs.list returned no inventory".into())
}
fn terminal(job: &Value) -> bool {
    matches!(
        job["status"].as_str(),
        Some("done" | "failed" | "cancelled")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn job(id: &str, session: &str, status: &str, cancellable: bool) -> Value {
        json!({"job":id,"name":"asset.import","session":session,"status":status,"cancellable":cancellable})
    }
    #[test]
    fn final_inventory_only_owns_new_jobs_from_this_human_session() {
        let mut resources = Resources::default();
        let baseline = job("jb_baseline", "human:run", "running", true);
        resources
            .baseline("human:run", &json!({"jobs":[baseline]}))
            .unwrap();
        let foreign = job("jb_foreign", "agent:other", "running", true);
        let owned = job("jb_owned", "human:run", "running", true);
        resources
            .observe(&json!({"jobs":[baseline,foreign,owned]}))
            .unwrap();
        resources.shell(json!({"busy":false}));
        resources.host_unsaved = Some(false);
        let action = serde_json::to_value(resources.next().unwrap()).unwrap();
        assert_eq!(action["method"], "jobs.cancel");
        assert_eq!(action["params"]["job"], "jb_owned");
        assert_eq!(resources.owned.len(), 1);
        assert!(!resources.drained());
        resources
            .observe(
                &json!({"jobs":[baseline,foreign,job("jb_owned","human:run","cancelled",false)]}),
            )
            .unwrap();
        assert!(resources.drained());
        assert_eq!(resources.cancelled, BTreeSet::from(["jb_owned".into()]));
        resources.shell(json!({"busy":true,"build":true}));
        assert!(
            !resources.drained(),
            "A terminal job table does not imply a shell export finished"
        );
        assert_eq!(
            serde_json::to_value(resources.next().unwrap()).unwrap()["method"],
            "jobs.list"
        );
        resources.shell(json!({"busy":false}));
        assert!(resources.drained());
    }
    #[test]
    fn missing_baseline_and_disappearing_owned_jobs_fail_closed() {
        let mut resources = Resources::default();
        assert!(resources.observe(&json!({"jobs":[]})).is_err());
        resources
            .baseline("human:run", &json!({"jobs":[]}))
            .unwrap();
        resources
            .observe(&json!({"jobs":[job("jb_owned","human:run","running",false)]}))
            .unwrap();
        let action = serde_json::to_value(resources.next().unwrap()).unwrap();
        assert_eq!(
            action["method"], "jobs.status",
            "Never cancel committing work"
        );
        resources.observe(&json!({"jobs":[]})).unwrap();
        assert!(!resources.errors.is_empty());
        assert!(!resources.drained());
    }
}
