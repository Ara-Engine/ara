//! Optional lab publication handshake, entirely on the transport worker until apply.
use anyhow::{Context, Result};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub(super) struct Gate {
    directory: PathBuf,
    requested: String,
}
pub(super) struct Staged {
    directory: PathBuf,
    revision: String,
}

impl Gate {
    pub(super) fn connect(
        contract: &std::path::Path,
        baseline: &crate::Contract,
    ) -> Result<Option<Self>> {
        let Some(directory) = std::env::var_os("ARA_HOTPATCH_LAB_CONTROL") else {
            return Ok(None);
        };
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        let owner = std::env::var("ARA_HOTPATCH_LAB_OWNER_PID")?.parse::<u32>()?;
        std::fs::write(
            directory.join("host.json"),
            serde_json::to_vec(&json!({
                "pid":std::process::id(), "owner_pid":owner, "contract":contract,
                "control":directory.parent().context("lab control parent")?,
            }))?,
        )?;
        std::fs::write(
            directory.join("applied.json"),
            serde_json::to_vec(
                &json!({"pid":std::process::id(),"revision":baseline.revision()?,"generation":0}),
            )?,
        )?;
        Ok(Some(Self {
            directory,
            requested: String::new(),
        }))
    }

    pub(super) fn request(&mut self) -> bool {
        let Ok(request) = std::fs::read_to_string(self.directory.join("build")) else {
            return false;
        };
        if request == self.requested {
            return false;
        }
        self.requested = request;
        true
    }

    pub(super) fn rejected(&self, revision: Option<&str>, reason: &str) {
        if let Ok(bytes) = serde_json::to_vec(
            &json!({"revision":revision,"reason":reason,"pid":std::process::id()}),
        ) {
            let _ = std::fs::write(self.directory.join("rejected.json"), bytes);
        }
    }

    pub(super) fn stage(&self, revision: String) -> Result<Option<Staged>> {
        // Each build gets an exact revision permit. An old permit cannot authorize
        // newer code, and no filesystem access is added to the idle frame check.
        let staged = self.directory.join("staged.json");
        std::fs::write(
            &staged,
            serde_json::to_vec(&json!({"revision":revision,"pid":std::process::id()}))?,
        )?;
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(2) {
            if std::fs::read_to_string(self.directory.join("publish"))
                .is_ok_and(|text| text == revision)
            {
                return Ok(Some(Staged {
                    directory: self.directory.to_owned(),
                    revision,
                }));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(None)
    }
}

impl Staged {
    pub(super) fn applied(self, generation: u64) {
        let result = serde_json::to_vec(&json!({"revision":self.revision,"pid":std::process::id(),"generation":generation,"at_us":crate::timestamp_us()}))
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(std::fs::write(self.directory.join("applied.json"), bytes)?));
        if let Err(error) = result {
            eprintln!("Rust hotpatch lab acknowledgement failed: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_exact_revision_permit_publishes_and_reports_a_generation() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = Gate {
            directory: directory.path().to_owned(),
            requested: String::new(),
        };
        assert!(!gate.request());
        std::fs::write(directory.path().join("build"), "new-source").unwrap();
        assert!(gate.request());
        assert!(!gate.request());
        std::fs::write(directory.path().join("publish"), "older-source").unwrap();
        assert!(gate.stage("new-source".into()).unwrap().is_none());
        std::fs::write(directory.path().join("publish"), "new-source").unwrap();
        gate.stage("new-source".into()).unwrap().unwrap().applied(7);
        let row: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("applied.json")).unwrap())
                .unwrap();
        assert_eq!(row["revision"], "new-source");
        assert_eq!(row["generation"], 7);
    }
}
