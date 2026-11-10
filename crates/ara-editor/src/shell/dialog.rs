//! Small modal dialogs: information (About, System Info, Connect an Agent in
//! `agents_connect`, "Why this value?") and one-field input (Save Layout As).
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
    /// Ara's identity card: logo, version, build, GPU and credits.
    About,
    /// The render device: identity, features and limits; Copy gives the full report.
    System,
    /// Connect an agent: per-client detection and setup (`agents_connect`).
    Agents,
    /// Name a layout to save under `.ara/layouts`.
    SaveLayout { name: String },
    /// Read-only background inventory, followed by an explicit module/IDE choice.
    ScriptModules {
        paths: Vec<String>,
        selected: Option<String>,
        pending: bool,
        error: Option<String>,
    },
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
        Self::About
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
        match self {
            Self::SaveLayout { .. } => Some("dialog-field"),
            Self::ScriptModules { paths, .. } if !paths.is_empty() => Some("script-module-choice"),
            Self::ScriptModules { .. } => Some("dialog-cancel"),
            Self::Info { .. } | Self::About | Self::System | Self::Agents => None,
        }
    }
}
impl Shell {
    pub(super) fn dialog_markup(&mut self, dialog: &Dialog, out: &mut String) {
        let [w, h] = self.ui.size;
        // Wider than popovers: dialogs carry paragraphs, paths and detail tables.
        let width = 440f32.min((w - 16.).max(1.));
        let height = (h - 16.).max(1.);
        let action = match dialog {
            Dialog::SaveLayout { .. } => "dialog-save",
            Dialog::ScriptModules { .. } => "dialog-script-open",
            Dialog::Info { .. } | Dialog::About | Dialog::System | Dialog::Agents => "dialog-close",
        };
        out.push_str(&format!(
            "<modal id='dialog' class='kit-dialog-layer' default-action='{action}' style='position:absolute;left:0px;top:0px;width:{w}px;height:{h}px;align-items:center;justify-content:center'><panel id='dialog-backdrop' class='kit-dialog-backdrop' style='position:absolute;left:0px;top:0px;width:{w}px;height:{h}px;background:var(--overlay)' /><panel id='dialog-body' class='dialog' style='position:relative;width:{width}px;max-height:{height}px;overflow:hidden'>"
        ));
        match dialog {
            Dialog::About => self.about_markup(out),
            Dialog::System => self.system_markup(out),
            Dialog::Agents => self.agents_markup(out),
            Dialog::Info { title, lines, code } => {
                out.push_str(&format!(
                    "<text id='dialog-title' class='title' text='{}' />",
                    escape(title)
                ));
                // The dialog sizes to its content, so the scroll keeps its auto basis
                // (editor scrolls default to 0 px and would collapse here).
                out.push_str("<scroll id='dialog-content' style='flex-basis:auto;min-height:0px;flex-shrink:1;gap:8px'>");
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
                self.dialog_buttons(code.is_some().then_some("Copy"), out);
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
            Dialog::ScriptModules {
                paths,
                selected,
                pending,
                error,
            } => {
                out.push_str("<text id='dialog-title' class='title' style='white-space:normal;flex-shrink:0' text='Open Scripts in IDE' /><scroll id='script-module-content' style='flex-basis:auto;flex-shrink:1;min-height:0px;min-width:0px'>");
                let note = if *pending {
                    "Discovering C# modules in the background. You can cancel this chooser; discovery will finish without opening an IDE."
                } else if paths.is_empty() && error.is_none() {
                    "This project has no usable C# module. Use Assets > Create > C# Script, then refresh this list."
                } else {
                    "Choose a module path, then select Open. Discovery does not change project documents."
                };
                out.push_str(&format!("<text id='script-module-status' class='dialog-text' style='flex-shrink:0' text='{}' />", escape(note)));
                if let Some(error) = error {
                    out.push_str(&format!("<text id='script-module-error' class='dialog-text' style='flex-shrink:0' text='{}' /><text id='script-module-last-good' class='dialog-text' style='flex-shrink:0' text='The last good module list is retained. Refresh to retry, or inspect Problems for details.' />", escape(error)));
                }
                if !paths.is_empty() {
                    out.push_str("<panel id='script-module-control' style='min-width:0px;flex-shrink:0'><text id='script-module-label' class='dialog-text' text='C# module' />");
                    self.choice_button(
                        "script-module-choice",
                        "C# module path",
                        choices::Select {
                            destination: choices::Destination::ScriptModule,
                            options: paths
                                .iter()
                                .map(|path| (Value::String(path.clone()), path.clone()))
                                .collect(),
                            current: selected
                                .as_ref()
                                .map_or(Value::Null, |path| Value::String(path.clone())),
                        },
                        out,
                    );
                    out.push_str("</panel>");
                }
                out.push_str("</scroll><panel id='dialog-actions' class='row' style='justify-content:end;flex-wrap:wrap;flex-shrink:0'>");
                out.push_str(&self.bind_button(
                    "dialog-script-refresh",
                    "Refresh",
                    Binding::DialogAction("script-refresh"),
                    "",
                ));
                out.push_str(&self.bind_button(
                    "dialog-cancel",
                    "Cancel",
                    Binding::DialogAction("close"),
                    "",
                ));
                let button = self.bind_button(
                    "dialog-script-open",
                    "Open",
                    Binding::DialogAction("script-open"),
                    "primary",
                );
                if *pending || selected.is_none() {
                    out.push_str(&button.replacen("<button ", "<button disabled='true' ", 1));
                } else {
                    out.push_str(&button);
                }
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
            ("script-open", Some(Dialog::ScriptModules { .. })) => {
                self.confirm_script_module(editor)?;
            }
            ("script-refresh", Some(Dialog::ScriptModules { .. })) => {
                self.refresh_script_modules(editor)?;
            }
            (_, Some(Dialog::ScriptModules { .. })) => self.cancel_script_module_intent(),
            (
                "copy",
                Some(Dialog::Info {
                    code: Some(code), ..
                }),
            ) => {
                self.copy_text(code);
                self.status_note = Some("Copied to the clipboard".into());
            }
            ("copy", Some(Dialog::About)) => {
                self.copy_text(
                    about_details(&self.device_info)
                        .iter()
                        .map(|(k, v)| format!("{k}: {v}"))
                        .collect::<Vec<_>>()
                        .join(
                            "
",
                        ),
                );
                self.status_note = Some("Copied to the clipboard".into());
            }
            ("copy", Some(Dialog::System)) => {
                self.copy_text(self.device_info.clone());
                self.status_note = Some("Copied the full device report".into());
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

use crate::about::{Detail, about_details, parse_device, system_sections};

/// A well of label/value rows (`{id}-details`); unavailable values read muted.
fn details_markup(out: &mut String, id: &str, key_class: &str, rows: Vec<Detail>) {
    out.push_str(&format!("<panel id='{id}-details' class='about-details'>"));
    for (n, (key, value, available)) in rows.into_iter().enumerate() {
        let value_class = if available {
            "about-value"
        } else {
            "about-value about-value-off"
        };
        out.push_str(&format!("<panel id='{id}-row-{n}' class='about-row'><text id='{id}-key-{n}' class='{key_class}' text='{key}' /><text id='{id}-value-{n}' class='{value_class}' text='{}' /></panel>", escape(&value)));
    }
    out.push_str("</panel>");
}

impl Shell {
    /// Close, after an optional Copy that copies what the dialog shows.
    pub(super) fn dialog_buttons(&mut self, copy: Option<&str>, out: &mut String) {
        out.push_str(
            "<panel id='dialog-actions' class='row' style='justify-content:end;flex-shrink:0'>",
        );
        if let Some(label) = copy {
            out.push_str(&self.bind_button(
                "dialog-copy",
                label,
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

    fn about_markup(&mut self, out: &mut String) {
        out.push_str("<panel id='about-hero' class='about-hero'><image id='about-logo' class='about-logo' src='brand-logo' /><panel id='about-heading' class='about-heading'><text id='dialog-title' class='about-name' text='Ara' />");
        out.push_str(&format!(
            "<text id='about-tagline' class='dialog-text' text='{}' /></panel></panel>",
            escape("A Rust game engine built by agents, for agents and humans.")
        ));
        let rows = about_details(&self.device_info)
            .into_iter()
            .map(|(key, value)| (key, value, true))
            .collect();
        details_markup(out, "about", "about-key", rows);
        out.push_str(&format!(
            "<text id='about-credits' class='caption' style='white-space:normal' text='{}' />",
            escape(
                "Inter and JetBrains Mono (OFL), Lucide icons (ISC). See THIRD_PARTY_NOTICES.md."
            )
        ));
        self.dialog_buttons(Some("Copy details"), out);
    }

    fn system_markup(&mut self, out: &mut String) {
        out.push_str("<text id='dialog-title' class='title' text='System info' />");
        let Some(info) = parse_device(&self.device_info) else {
            out.push_str("<text id='dialog-line-0' class='dialog-text' text='The renderer has not started yet, so there is no device to report.' />");
            self.dialog_buttons(None, out);
            return;
        };
        out.push_str(&format!(
            "<text id='dialog-line-0' class='dialog-text' text='{}' />",
            escape(
                "The editor and every viewport render on this GPU. Copy gives the full device report."
            )
        ));
        // Content height, scrolling only when the window is shorter than the report.
        out.push_str(
            "<scroll id='dialog-content' style='flex-basis:auto;min-height:0px;flex-shrink:1;gap:10px'>",
        );
        for (id, title, rows) in system_sections(&info) {
            out.push_str(&format!("<panel id='{id}' class='about-section'><text id='{id}-title' class='about-section-title' text='{title}' />"));
            details_markup(out, id, "about-key about-key-wide", rows);
            out.push_str("</panel>");
        }
        out.push_str("</scroll>");
        self.dialog_buttons(Some("Copy full report"), out);
    }
}
