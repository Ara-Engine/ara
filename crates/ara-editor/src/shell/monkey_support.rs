//! Read-only lifecycle evidence for the isolated native runner.
use super::Shell;
use serde_json::{Value, json};

impl Shell {
    /// Work without a Host job record must finish before the runner can claim a
    /// clean drain. Live preview loops are instead joined during Shell teardown.
    pub(crate) fn monkey_background_work(&self) -> Value {
        // This Instant is the last poll, not the task's start. Whole-second
        // precision avoids emitting a changed full inventory every native frame.
        let script_job = self.script_job.as_ref().map(|(job, last)| {
            json!({"job":job,"ageMs":last.elapsed().as_millis()/1000*1000,"ageResolutionMs":1000,
                "ageScope":"Since handle assignment or last status poll; not task lifetime"})
        });
        json!({"build":self.build.is_some(),"runBuild":self.run.is_some(),"script":self.script_job.is_some(),"scriptJob":script_job,"scriptInventory":self.script_module_job_report(),
            "busy":self.build.is_some() || self.run.is_some() || self.script_job.is_some() || self.script_module_busy()})
    }
}
