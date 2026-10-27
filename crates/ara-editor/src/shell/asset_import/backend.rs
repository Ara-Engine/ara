//! Adapt the generic jobs API; optional import details never replace authority state.
use serde_json::{Value, json};
mod preflight;
pub(super) use preflight::{Plan, Preflight, preflight};

pub(super) fn authored_folder(path: &str) -> bool {
    path.is_empty()
        || (!path.contains([':', '\\'])
            && !path.starts_with('/')
            && path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."))
}
pub(super) fn accepted_job(value: &Value) -> Option<&str> {
    value["job"].as_str().or_else(|| {
        value["outputs"]
            .as_array()?
            .iter()
            .find_map(|output| output["job"].as_str())
    })
}

#[derive(PartialEq)]
pub(super) struct Status {
    raw: Value,
    pub state: String,
    pub message: String,
    pub progress: f32,
    pub phase: Option<String>,
}
impl Status {
    pub fn preparing() -> Self {
        Self::read(json!({"status":"running","progress":0,"message":"Preparing import…"}))
    }
    pub fn read(raw: Value) -> Self {
        let state = raw["status"].as_str().unwrap_or("running").to_owned();
        let phase = raw["phase"].as_str().map(str::to_owned);
        let progress_message = raw["message"]
            .as_str()
            .filter(|message| !message.is_empty());
        let message = (if state == "failed" {
            raw["diagnostics"][0]["message"]
                .as_str()
                .or(progress_message)
        } else if state == "cancelled" {
            Some("Import cancelled")
        } else {
            progress_message
        })
        .unwrap_or(match state.as_str() {
            "done" => "Import complete",
            "failed" => "Import failed",
            "cancelled" => "Import cancelled",
            _ => "Preparing import…",
        })
        .to_owned();
        let progress = raw["progress"]
            .as_f64()
            .filter(|value| value.is_finite())
            .unwrap_or(0.)
            .clamp(0., 1.) as f32;
        Self {
            raw,
            state,
            message,
            progress,
            phase,
        }
    }
    pub fn running(&self) -> bool {
        self.state == "running"
    }
    pub fn succeeded(&self) -> bool {
        self.state == "done"
    }
    pub fn can_cancel(&self) -> bool {
        self.running()
            && self.phase.as_deref() != Some("committing")
            && self.raw["cancellable"].as_bool().unwrap_or(true)
    }
    pub fn summary(&self) -> String {
        if !self.succeeded() {
            return self.message.clone();
        }
        let counts = ["created", "replaced", "skipped", "renamed"]
            .into_iter()
            .filter_map(|name| {
                self.raw["result"][name]
                    .as_array()
                    .map(|paths| format!("{} {name}", paths.len()))
            })
            .collect::<Vec<_>>();
        if counts.is_empty() {
            "Import complete".into()
        } else {
            format!("Import complete · {}", counts.join(" · "))
        }
    }
    /// Selection needs an explicit storage path; authored paths are not guessed.
    pub fn selection_path(&self) -> Option<&str> {
        self.raw["result"]["storagePath"].as_str()
    }
    pub fn value(&self) -> &Value {
        &self.raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generic_jobs_remain_usable_and_commit_phase_disables_cancel() {
        let preparing = Status::read(
            json!({"status":"running","progress":0.5,"message":"Staging files · 3 / 5 · 12 / 20 MB"}),
        );
        assert!(preparing.can_cancel());
        assert_eq!(preparing.progress, 0.5);
        let committing = Status::read(
            json!({"status":"running","phase":"committing","progress":0.9,"cancellable":true}),
        );
        assert!(!committing.can_cancel());
        let done = Status::read(
            json!({"status":"done","result":{"created":[{"path":"Art/a.glb","id":"as_one"}],"skipped":[],"txn":"tx_one"}}),
        );
        assert!(done.succeeded());
        assert!(
            done.selection_path().is_none(),
            "an authored path must not be used as browser storage"
        );
        assert_eq!(done.summary(), "Import complete · 1 created · 0 skipped");
        let failed = Status::read(
            json!({"status":"failed","message":"Staging files…","diagnostics":[{"message":"A source is no longer readable"}]}),
        );
        assert_eq!(failed.summary(), "A source is no longer readable");
    }
    #[test]
    fn authored_folder_validation_is_independent_of_project_root_names() {
        for path in ["", "Art", "Custom Content/Models", "Game.UI/Textures"] {
            assert!(authored_folder(path), "{path}");
        }
        for path in [
            "../Assets",
            "/absolute",
            "C:/Assets",
            "Art\\Models",
            "Art//Models",
            "Art/../Models",
        ] {
            assert!(!authored_folder(path), "{path}");
        }
    }
}
