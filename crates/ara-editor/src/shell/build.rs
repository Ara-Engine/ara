//! Build > Build Current Profile: export the project with one of its presets
//! (`presets/<name>.export.json`) through `ara-export` on a worker thread. The status
//! bar shows the stage; warnings and errors (with their fix commands) go to the
//! diagnostics bus, so Problems and `ara logs` show them; the result opens in
//! Explorer.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use super::*;

/// What the export thread reports to the window.
pub(crate) enum BuildEvent {
    /// A stage started (`templates`, `content`, `package`, ...).
    Stage(String),
    /// A warning or error, as bus-ready diagnostic JSON.
    Diagnostic(Value),
    /// The export ended: the output folder, or why it failed.
    Finished(Result<PathBuf, String>),
}

/// A running export.
pub(crate) struct BuildJob {
    events: Receiver<BuildEvent>,
    /// Preset name, for the status bar.
    pub preset: String,
    /// Current stage.
    pub stage: String,
}

struct Sink(Sender<BuildEvent>);

impl ara_export::event::EventSink for Sink {
    fn emit(&mut self, event: &ara_export::event::Event) -> std::io::Result<()> {
        use ara_export::event::EventKind;
        let message = match &event.kind {
            EventKind::Progress { status, .. } if status == "started" => {
                Some(BuildEvent::Stage(event.stage.clone()))
            }
            EventKind::Warning(d) | EventKind::Error(d) => {
                let error = matches!(event.kind, EventKind::Error(_));
                let code = ara_diag::catalog::lookup(&d.code)
                    .map(|doc| doc.code)
                    .unwrap_or(crate::codes::ACTION);
                let mut message = d.message.clone();
                if let Some(fix) = d.fixes.first() {
                    message.push_str(&format!(" Fix: {fix}"));
                }
                let diagnostic = if error {
                    ara_diag::Diagnostic::error(code, message)
                } else {
                    ara_diag::Diagnostic::warning(code, message)
                }
                .subject(ara_diag::Subject::system("export"));
                serde_json::to_value(&diagnostic)
                    .ok()
                    .map(BuildEvent::Diagnostic)
            }
            _ => None,
        };
        if let Some(message) = message {
            let _ = self.0.send(message);
        }
        Ok(())
    }
}

/// The project's export presets (`presets/*.export.json`), by name.
pub(crate) fn presets(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("presets"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_suffix(".export.json")
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

impl Shell {
    /// Start exporting with the first preset (Build > Build Current Profile).
    pub(crate) fn build_current(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.build.is_some() {
            return Err("an export is already running".into());
        }
        let root = editor.host.root().to_owned();
        let preset = presets(&root).into_iter().next().ok_or(
            "No export preset: add presets/windows-portable.export.json (ara-export preset --kind windows-portable --base <platform preset> --output presets/windows-portable.export.json)",
        )?;
        editor.host.flush();
        let text =
            std::fs::read_to_string(root.join("presets").join(format!("{preset}.export.json")))?;
        let parsed = ara_export::ExportPreset::from_text(&text).map_err(|e| e.to_string())?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let output = root.join("builds").join(format!("{preset}-{stamp}"));
        let (send, events) = channel();
        let request = ara_export::ExportRequest {
            project: root.clone(),
            preset: parsed,
            output: output.clone(),
            cache: root.join(".ara/export-cache"),
            device: None,
            allow_emulator: false,
        };
        std::thread::spawn(move || {
            let mut sink = Sink(send.clone());
            let result = ara_export::export(&request, &mut sink)
                .map(|_| request.output.clone())
                .map_err(|e| format!("{e:#}"));
            let _ = send.send(BuildEvent::Finished(result));
        });
        self.build = Some(BuildJob {
            events,
            preset: preset.clone(),
            stage: "starting".into(),
        });
        self.status_note = Some(format!("Exporting with {preset}…"));
        Ok(())
    }

    /// Drain export events at a frame boundary; true when the UI should rebuild.
    pub(crate) fn poll_build(&mut self, editor: &mut Editor) -> bool {
        let Some(job) = &mut self.build else {
            return false;
        };
        let mut changed = false;
        let mut finished = None;
        while let Ok(event) = job.events.try_recv() {
            changed = true;
            match event {
                BuildEvent::Stage(stage) => {
                    job.stage = stage;
                    self.status_note = Some(format!("Exporting ({}): {}…", job.preset, job.stage));
                }
                BuildEvent::Diagnostic(diagnostic) => {
                    let _ = editor.action(
                        "diag.report",
                        json!({"diagnostics": [diagnostic], "engine": true}),
                    );
                }
                BuildEvent::Finished(result) => finished = Some(result),
            }
        }
        if let Some(result) = finished {
            self.build = None;
            match result {
                Ok(output) => {
                    self.status_note = Some(format!("Exported to {}", output.display()));
                    let _ = ops::open_external(&output);
                }
                Err(error) => {
                    self.status_note = Some("Export failed: see Problems".into());
                    editor.report(
                        "action",
                        &format!("Export failed: {error}"),
                        Some("system:export"),
                    );
                }
            }
        }
        changed
    }
}
