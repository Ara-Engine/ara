//! Read-only lifecycle evidence for the isolated native runner.
use super::Shell;
use serde_json::{Value, json};

impl Shell {
    /// Work without a Host job record must finish before the runner can claim a
    /// clean drain. Live preview loops are instead joined during Shell teardown.
    pub(crate) fn monkey_background_work(&self) -> Value {
        json!({"build":self.build.is_some(),"runBuild":self.run.is_some(),"script":self.script_job.is_some(),
            "busy":self.build.is_some() || self.run.is_some() || self.script_job.is_some()})
    }
}
