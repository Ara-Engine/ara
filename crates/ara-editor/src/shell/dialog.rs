//! Small modal dialogs: information (About, System Info, Connect an Agent,
//! "Why this value?") and one-field input (Save Layout As).
use super::*;

/// A modal dialog; Esc or Close dismisses it.
#[derive(Clone, Debug)]
pub(crate) enum Dialog {
    /// Read-only text with an optional copyable block.
    Info {
        title: String,
        lines: Vec<String>,
        code: Option<String>,
    },
    /// Name a layout to save under `.ara/layouts`.
    SaveLayout { name: String },
}
impl Dialog {
    /// Review an external tool fix without starting a process while a project is pinned.
    pub fn cli_fix(title: &str, args: &[String]) -> Self {
        fn quote(arg: &str) -> String {
            if arg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
                && !arg.is_empty()
            {
                arg.to_owned()
            } else if cfg!(windows) {
                format!("'{}'", arg.replace('\'', "''"))
            } else {
                format!("'{}'", arg.replace('\'', "'\\''"))
            }
        }
        let repair = matches!(args, [hub, repair, _] if hub == "hub" && repair == "repair");
        Self::Info {
            title: title.to_owned(),
            lines: if repair {
                vec!["Save your work and close this pinned project before repairing its engine installation.".into(), "Copy this command, close the project, then run it in a terminal. Repair verifies the signed engine package and waits for the Hub service job.".into()]
            } else {
                vec!["Review and copy this command, then run it in a terminal.".into()]
            },
            code: Some(
                std::iter::once("ara".to_owned())
                    .chain(args.iter().map(|arg| quote(arg)))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        }
    }

    pub fn about() -> Self {
        Self::Info {
            title: "About Ara".into(),
            lines: vec![
                format!("Ara {}", env!("CARGO_PKG_VERSION")),
                "A Rust game engine built by agents, for agents and humans.".into(),
                "Inter and JetBrains Mono (OFL), Lucide icons (ISC). See THIRD_PARTY_NOTICES.md."
                    .into(),
            ],
            code: None,
        }
    }
    pub fn system(device: &str) -> Self {
        Self::Info {
            title: "System info".into(),
            lines: vec!["The editor and every viewport share this Vulkan device.".into()],
            code: Some(if device.is_empty() {
                "Device information is not available yet.".into()
            } else {
                device.to_owned()
            }),
        }
    }
    pub fn connect(editor: &Editor) -> Self {
        Self::Info {
            title: "Connect an agent".into(),
            lines: vec![
                "Claude Code, Codex and other MCP clients use the same commands as this editor (editor.commands).".into(),
                "Run this in the project folder, or add it to the client's MCP config:".into(),
            ],
            code: Some(format!("ara mcp --root \"{}\"", editor.host.root().display())),
        }
    }
    pub fn provenance(field: &crate::inspector::Field) -> Self {
        use crate::inspector::Provenance;
        let source = match field.provenance {
            Provenance::Default => "No authored value: the schema default applies.",
            Provenance::Prefab => "Inherited from the prefab document.",
            Provenance::Override => "An instance override of the prefab value.",
            Provenance::Authored => "Authored on this entity in the scene.",
            Provenance::AgentChanged => "Changed by an agent; review it in Agents.",
        };
        let mut lines = vec![source.to_owned()];
        if let Some(was) = &field.was {
            lines.push(format!(
                "Before the agent change: {}",
                super::panels::display(was)
            ));
        }
        if !field.doc.is_empty() {
            lines.push(field.doc.clone());
        }
        Self::Info {
            title: format!("Why is {} this value?", field.label),
            lines,
            code: Some(format!(
                "{} = {}",
                field.path,
                super::panels::display(&field.value)
            )),
        }
    }
    pub fn save_layout() -> Self {
        Self::SaveLayout {
            name: String::new(),
        }
    }
    /// The text field that takes focus, if any.
    pub fn field(&self) -> Option<&'static str> {
        matches!(self, Self::SaveLayout { .. }).then_some("dialog-field")
    }
}
impl Shell {
    pub(super) fn dialog_markup(&mut self, dialog: &Dialog, out: &mut String) {
        let [w, h] = self.ui.size;
        let width = ara_ui::popover::MAX_WIDTH.min((w - 16.).max(1.));
        let height = (h - 16.).max(1.);
        let action = if matches!(dialog, Dialog::SaveLayout { .. }) {
            "dialog-save"
        } else {
            "dialog-close"
        };
        out.push_str(&format!(
            "<modal id='dialog' class='kit-dialog-layer' default-action='{action}' style='position:absolute;left:0px;top:0px;width:{w}px;height:{h}px;align-items:center;justify-content:center'><panel id='dialog-backdrop' class='kit-dialog-backdrop' style='position:absolute;left:0px;top:0px;width:{w}px;height:{h}px;background:var(--overlay)' /><panel id='dialog-body' class='dialog' style='position:relative;width:{width}px;max-height:{height}px;overflow:hidden'>"
        ));
        match dialog {
            Dialog::Info { title, lines, code } => {
                out.push_str(&format!(
                    "<text id='dialog-title' class='title' text='{}' />",
                    escape(title)
                ));
                out.push_str("<scroll id='dialog-content' style='min-height:0px;flex-shrink:1'>");
                for (n, line) in lines.iter().enumerate() {
                    out.push_str(&format!(
                        "<text id='dialog-line-{n}' class='dialog-text' text='{}' />",
                        escape(line)
                    ));
                }
                if let Some(code) = code {
                    out.push_str(&format!(
                        "<text id='dialog-code' class='dialog-code' text='{}' />",
                        escape(code)
                    ));
                }
                out.push_str("</scroll>");
                out.push_str("<panel id='dialog-actions' class='row' style='justify-content:end'>");
                if code.is_some() {
                    out.push_str(&self.bind_button(
                        "dialog-copy",
                        "Copy",
                        Binding::DialogAction("copy"),
                        "",
                    ));
                }
                out.push_str(&self.bind_button(
                    "dialog-close",
                    "Close",
                    Binding::DialogAction("close"),
                    "primary",
                ));
                out.push_str("</panel>");
            }
            Dialog::SaveLayout { name } => {
                self.bindings.insert(
                    ara_ui::source::stable_id("dialog-field"),
                    Binding::DialogText,
                );
                out.push_str(&format!("<text id='dialog-title' class='title' text='Save layout as' /><text id='dialog-line' class='dialog-text' text='Saves panels, sizes and floating windows to .ara/layouts. Load it from Window › Layouts.' /><field id='dialog-field' text='{}' label='Layout name' action='Dispatch' />",escape(name)));
                out.push_str("<panel id='dialog-actions' class='row' style='justify-content:end'>");
                out.push_str(&self.bind_button(
                    "dialog-cancel",
                    "Cancel",
                    Binding::DialogAction("close"),
                    "",
                ));
                out.push_str(&self.bind_button(
                    "dialog-save",
                    "Save",
                    Binding::DialogAction("save"),
                    "primary",
                ));
                out.push_str("</panel>");
            }
        }
        out.push_str("</panel></modal>");
    }
    /// Run a dialog button.
    pub(super) fn dialog_action(
        &mut self,
        editor: &mut Editor,
        action: &str,
    ) -> ara_ui::Result<()> {
        match (action, self.dialog.clone()) {
            (
                "copy",
                Some(Dialog::Info {
                    code: Some(code), ..
                }),
            ) => {
                self.copy_text(code);
                self.status_note = Some("Copied to the clipboard".into());
            }
            ("save", Some(Dialog::SaveLayout { name })) => {
                editor.action("editor.layout.save", json!({"name": name.trim()}))?;
                self.status_note = Some(format!("Saved layout “{}”", name.trim()));
                self.dialog = None;
            }
            _ => self.dialog = None,
        }
        Ok(())
    }
}
