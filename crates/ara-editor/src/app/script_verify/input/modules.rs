//! Native palette, shared module Select, and explicit Open with IDE suppression.
use super::*;
use crate::shell::Dialog;
impl ScriptCheck {
    pub(super) fn modules(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !self.palette(shell, window, scale, "tools.cs.solution", events)? {
            return Ok(());
        }
        match self.phase {
            4 => {
                let Some(Dialog::ScriptModules {
                    paths,
                    pending: false,
                    ..
                }) = &shell.dialog
                else {
                    return Ok(());
                };
                let source = self
                    .source
                    .as_ref()
                    .ok_or("Native J10 source path missing")?
                    .strip_prefix(editor.host.root())?
                    .to_string_lossy()
                    .replace('\\', "/");
                self.module = paths
                    .iter()
                    .filter(|path| path.is_empty() || source.starts_with(&format!("{path}/")))
                    .max_by_key(|path| path.len())
                    .cloned();
                if self.module.is_none() {
                    return Err("Discovered module list does not contain Spinner.cs".into());
                }
                self.click(shell, window, scale, "script-module-choice", events)?;
                self.next();
            }
            5 => {
                let module = self.module.as_deref().ok_or("J10 module choice missing")?;
                let option = shell
                    .ui
                    .nodes
                    .iter()
                    .find(|node| {
                        node.element
                            .id
                            .starts_with("choice-script-module-choice-option-")
                            && node
                                .element
                                .attrs
                                .get("label")
                                .is_some_and(|label| label == module)
                    })
                    .map(|node| node.element.id.clone());
                let Some(option) = option else { return Ok(()) };
                if self.reveal(shell, window, scale, &option, events)? {
                    self.click(shell, window, scale, &option, events)?;
                    self.next();
                }
            }
            6 => {
                let selected = matches!(&shell.dialog,Some(Dialog::ScriptModules{selected:Some(path),pending:false,..}) if Some(path)==self.module.as_ref());
                if !selected {
                    return Ok(());
                }
                self.pending_capture = Some("module-selected-before-open");
                self.next();
            }
            7 => {
                self.click(shell, window, scale, "dialog-script-open", events)?;
                self.next();
            }
            _ => {
                if shell.dialog.is_some() {
                    return Ok(());
                }
                let note = shell
                    .status_note
                    .as_deref()
                    .ok_or("Explicit module Open produced no status")?;
                if !note.contains("IDE launch is disabled")
                    || !note.contains(self.module.as_deref().unwrap_or("<missing>"))
                {
                    return Err(format!(
                        "Explicit module Open did not acknowledge the selected module: {note}"
                    )
                    .into());
                }
                let job = shell.script_module_job_report();
                if job["running"] != false
                    || job["workerRetired"] != true
                    || job["pendingPresentation"] != false
                {
                    return Ok(());
                }
                let invocation = shell.script_ide_request();
                let target = editor
                    .host
                    .root()
                    .join(
                        self.module
                            .as_deref()
                            .ok_or("J10 selected module missing")?,
                    )
                    .canonicalize()?;
                let target = target
                    .to_str()
                    .ok_or("Selected module path is not absolute UTF-8")?;
                if invocation["targetFile"] != target
                    || invocation["line"] != Value::Null
                    || invocation["column"] != Value::Null
                    || invocation["launchSuppressed"] != true
                    || invocation["attempted"] != false
                    || invocation["outcome"] != "suppressed"
                {
                    return Err(format!("Explicit module Open retained the wrong/surprisingly launched IDE invocation: {invocation}").into());
                }
                self.done(json!({"module":self.module,"explicitOpen":true,"status":note,"inventory":job,
                    "ideRequest":invocation,
                    "ingress":"SDL palette pointer, module Select pointer, explicit Open pointer",
                    "externalIDE":"suppressed by offscreen guard; no visible IDE launched by this journey"}));
            }
        }
        Ok(())
    }
}
