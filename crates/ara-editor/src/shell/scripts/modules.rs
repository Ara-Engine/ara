//! Single-flight read-only C# discovery. Only a completed worker is joined by UI polling.
use super::*;
use std::{thread::JoinHandle, time::Instant};

type Inventory = Result<Vec<PathBuf>, Vec<ara_diag::Diagnostic>>;

struct Job {
    worker: JoinHandle<Inventory>,
    started: Instant,
}

/// Last good inventory plus one bounded discovery task and its separate UI intent.
#[derive(Default)]
pub(crate) struct ModuleList {
    root: Option<PathBuf>,
    paths: Vec<PathBuf>,
    job: Option<Job>,
    intent: bool,
    error: Option<String>,
    started: u64,
    completed: u64,
    cancelled: u64,
}

impl ModuleList {
    fn start(&mut self, root: &Path) -> std::io::Result<bool> {
        self.start_with(root, modules)
    }

    fn start_with(
        &mut self,
        root: &Path,
        discover: impl FnOnce(&Path) -> Inventory + Send + 'static,
    ) -> std::io::Result<bool> {
        if self.job.is_some() {
            return Ok(false);
        }
        let job_root = root.to_owned();
        let worker = std::thread::Builder::new()
            .name("editor-script-inventory".into())
            .spawn(move || discover(&job_root))?;
        if self.root.as_deref() != Some(root) {
            self.paths.clear();
        }
        self.root = Some(root.to_owned());
        self.error = None;
        self.started += 1;
        self.job = Some(Job {
            worker,
            started: Instant::now(),
        });
        Ok(true)
    }

    fn poll(&mut self) -> Option<Vec<ara_diag::Diagnostic>> {
        if !self.job.as_ref()?.worker.is_finished() {
            return None;
        }
        let job = self.job.take()?;
        // is_finished is the only entry to this join; unfinished inventory never
        // blocks the window thread, including after the chooser is cancelled.
        let result = job.worker.join().unwrap_or_else(|_| {
            Err(vec![ara_diag::Diagnostic::error(
                crate::codes::ACTION,
                "C# module discovery worker panicked; retry discovery or see Problems",
            )])
        });
        self.completed += 1;
        Some(self.update(result))
    }

    fn update(&mut self, result: Inventory) -> Vec<ara_diag::Diagnostic> {
        match result {
            Ok(mut paths) => {
                paths.sort();
                paths.dedup();
                self.paths = paths;
                self.error = None;
                Vec::new()
            }
            Err(mut diagnostics) => {
                if diagnostics.is_empty() {
                    diagnostics.push(ara_diag::Diagnostic::error(
                        crate::codes::ACTION,
                        "C# module discovery failed without details; retry discovery or see Problems",
                    ));
                }
                self.error = Some(
                    diagnostics
                        .iter()
                        .map(|diagnostic| diagnostic.message.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                diagnostics
            }
        }
    }

    fn cancel_intent(&mut self) {
        if std::mem::take(&mut self.intent) {
            self.cancelled += 1;
        }
    }

    fn paths(&self) -> Vec<String> {
        self.paths
            .iter()
            .map(|path| {
                path.strip_prefix(self.root.as_deref().unwrap_or(Path::new("")))
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    fn dialog(&self, selected: Option<&str>) -> Dialog {
        let paths = self.paths();
        let selected = selected
            .filter(|selected| paths.iter().any(|path| path == selected))
            .map(str::to_owned)
            .or_else(|| paths.first().cloned());
        Dialog::ScriptModules {
            paths,
            selected,
            pending: self.job.is_some(),
            error: self.error.clone(),
        }
    }
}

impl Shell {
    /// Start read-only inventory off-thread, then explicitly choose what to open.
    /// Repeated requests share one task; cancelling the chooser never opens an IDE later.
    pub(crate) fn open_script_solution(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if matches!(self.dialog, Some(Dialog::ScriptModules { .. })) {
            return Ok(());
        }
        if self.dialog.is_some() || self.confirmations.is_open() || self.build_options_open() {
            return Err(
                "Finish or close the current dialog, then open Scripts in IDE again".into(),
            );
        }
        if self.script_modules.job.is_some()
            && self.script_modules.root.as_deref() != Some(editor.host.root())
        {
            return Err("The previous project's module discovery is still finishing; retry when it completes".into());
        }
        self.script_modules.start(editor.host.root())?;
        if editor.state.palette {
            editor.action("editor.palette", json!({"open":false}))?;
        }
        self.leave();
        self.menu = None;
        self.script_modules.intent = true;
        self.dialog = Some(self.script_modules.dialog(None));
        self.rebuild(editor)
    }

    /// Observe completed inventory without waiting for a worker or launching an IDE.
    pub(crate) fn poll_script_modules(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.script_modules.intent
            && (!matches!(self.dialog, Some(Dialog::ScriptModules { .. }))
                || self.script_modules.root.as_deref() != Some(editor.host.root()))
        {
            self.script_modules.cancel_intent();
        }
        let Some(diagnostics) = self.script_modules.poll() else {
            return Ok(false);
        };
        if !diagnostics.is_empty() {
            self.status_note = Some(
                "C# module discovery failed; the last good list is retained. See Problems.".into(),
            );
            editor.report_failure(&crate::editor::EditorError(
                ara_proto::RpcError::from_diagnostics(diagnostics),
            ));
        }
        if self.script_modules.intent {
            let selected = match &self.dialog {
                Some(Dialog::ScriptModules { selected, .. }) => selected.as_deref(),
                _ => None,
            };
            self.dialog = Some(self.script_modules.dialog(selected));
        }
        Ok(true)
    }

    pub(in crate::shell) fn refresh_script_modules(
        &mut self,
        editor: &Editor,
    ) -> ara_ui::Result<()> {
        if !matches!(self.dialog, Some(Dialog::ScriptModules { .. })) {
            return Ok(());
        }
        self.script_modules.start(editor.host.root())?;
        let selected = match &self.dialog {
            Some(Dialog::ScriptModules { selected, .. }) => selected.as_deref(),
            _ => None,
        };
        self.dialog = Some(self.script_modules.dialog(selected));
        Ok(())
    }

    /// Retained Select intent; choosing a module does not launch it.
    pub(crate) fn choose_script_module(&mut self, value: Value) -> ara_ui::Result<()> {
        let path = value.as_str().ok_or("Choose a C# module path")?;
        if let Some(Dialog::ScriptModules {
            paths, selected, ..
        }) = &mut self.dialog
            && paths.iter().any(|candidate| candidate == path)
        {
            *selected = Some(path.into());
        }
        Ok(())
    }

    pub(in crate::shell) fn confirm_script_module(
        &mut self,
        editor: &Editor,
    ) -> ara_ui::Result<()> {
        let Some(Dialog::ScriptModules {
            selected: Some(path),
            pending: false,
            ..
        }) = &self.dialog
        else {
            return Ok(());
        };
        if !self.script_modules.intent
            || self.script_modules.root.as_deref() != Some(editor.host.root())
            || !self.script_modules.paths().contains(path)
        {
            return Err("The module list changed; refresh it and choose a module again".into());
        }
        self.open_script_ide(editor, path)?;
        self.status_note = Some(if self.offscreen || !self.open_ide {
            format!("Selected C# module {path}; IDE launch is disabled for this session")
        } else {
            format!("Opened C# module {path}")
        });
        self.script_modules.intent = false;
        self.dialog = None;
        Ok(())
    }

    /// Cancel presentation only. An uninterruptible inventory task remains visible to drain checks.
    pub(crate) fn cancel_script_module_intent(&mut self) {
        self.script_modules.cancel_intent();
        if matches!(self.dialog, Some(Dialog::ScriptModules { .. })) {
            self.dialog = None;
        }
    }

    /// True until the single inventory worker has actually finished and been joined.
    pub(crate) fn script_module_busy(&self) -> bool {
        self.script_modules.job.is_some()
    }

    /// Resource evidence keeps cancellation, presentation and worker retirement distinct.
    pub(crate) fn script_module_job_report(&self) -> Value {
        json!({
            "running":self.script_module_busy(),
            "pendingPresentation":self.script_modules.intent,
            "workerRetired":!self.script_module_busy(),
            "started":self.script_modules.started,
            "completed":self.script_modules.completed,
            "cancelledPresentations":self.script_modules.cancelled,
            "lastGoodModules":self.script_modules.paths.len(),
            "lastError":self.script_modules.error,
            "elapsedMs":self.script_modules.job.as_ref().map(|job|job.started.elapsed().as_secs_f64()*1000.),
        })
    }
}

#[cfg(test)]
#[path = "modules/tests.rs"]
mod tests;
