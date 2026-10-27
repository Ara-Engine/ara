//! Script creation uses the same typed source commands as MCP. Inline rename
//! chooses the final declaration before the background compile and attachment.
use std::path::{Path, PathBuf};

use super::*;

struct NoClipboard;
impl ara_ui_text::Clipboard for NoClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}

/// The project's C# modules (folders with `ara.module.toml`), sorted.
pub(crate) fn modules(root: &Path) -> Vec<PathBuf> {
    ara_play_host::script_modules(root)
}

/// Open in VS Code when available, otherwise use the system's file association.
pub(crate) fn open_in_ide(folder: &Path, file: &Path) -> ara_ui::Result<()> {
    let code = if cfg!(windows) { "code.cmd" } else { "code" };
    if std::process::Command::new(code)
        .arg(folder)
        .arg(file)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    ops::open_external(file)
}

#[derive(Clone)]
pub(super) struct Rename {
    pub path: String,
    pub name: String,
    pub attach_to: Vec<String>,
}

impl Shell {
    /// Create a script in the targeted folder, then focus its inline name before compiling.
    pub(crate) fn create_script(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
        template: &str,
        attach_to: Vec<String>,
    ) -> ara_ui::Result<()> {
        let folder = match target {
            commands::Target::Folder(p) => p.clone(),
            commands::Target::Asset(p) => p.rsplit_once('/').map_or("", |(p, _)| p).to_owned(),
            _ => editor.state.asset_folder.clone(),
        };
        let stem = if template == "system" {
            "NewSystem"
        } else {
            "NewBehaviour"
        };
        let path =
            (1..)
                .map(|n| {
                    let name = if n == 1 {
                        stem.to_owned()
                    } else {
                        format!("{stem}{n}")
                    };
                    if folder.is_empty() {
                        format!("{name}.cs")
                    } else {
                        format!("{folder}/{name}.cs")
                    }
                })
                .find(|p| {
                    !editor.host.root().join(p).exists()
                        && !self.browser.items.iter().any(|a| {
                            a.kind == "cs" && names::file_name(&a.path) == names::file_name(p)
                        })
                })
                .ok_or("No unused script name")?;
        editor.action(
            "script.create",
            json!({"path":path,"template":template,"compile":false}),
        )?;
        self.browser.insert_path(&path, "cs");
        self.asset_offset = 0.;
        editor.action("editor.search", json!({"panel":"assets","query":""}))?;
        editor.action("editor.layout.show", json!({"panel":"assets"}))?;
        editor.action("editor.workspace", json!({"folder":folder,"asset":path}))?;
        self.asset_rename = Some(Rename {
            name: names::stem(names::file_name(&path)).into(),
            path,
            attach_to,
        });
        self.status_note = Some("Name your script, then press Enter to compile".into());
        Ok(())
    }

    pub(super) fn asset_rename_markup(&mut self, out: &mut String) {
        if let Some(rename) = &self.asset_rename {
            self.bindings.insert(
                ara_ui::source::stable_id("asset-rename-value"),
                Binding::AssetRenameValue,
            );
            out.push_str(&format!("<field id='asset-rename-value' style='position:absolute;left:0px;top:0px;width:160px;height:24px' text='{}' label='Script class name' description='Enter to create and compile; Escape keeps the default name' action='Dispatch' />",escape(&rename.name)));
        }
    }

    pub(super) fn position_asset_rename(&mut self) -> ara_ui::Result<()> {
        if let Some(rename) = &self.asset_rename
            && let Some(field) = self.ui.find("asset-rename-value")
        {
            let visible = self.shown_node("assets-content").is_some();
            self.ui.set_visible(field, visible)?;
            if !visible {
                return Ok(());
            }
            let card = self.ui.nodes.iter().find(|n|matches!(self.bindings.get(&ara_ui::source::stable_id(&n.element.id)),Some(Binding::Asset(a)) if a.path==rename.path));
            let rect = card
                .and_then(|node| {
                    self.ui
                        .find(&format!("{}-label", node.element.id))
                        .map(|i| self.ui.nodes[i].rect)
                })
                .or_else(|| card.map(|node| node.rect))
                .or_else(|| {
                    self.ui
                        .find("assets-content")
                        .map(|i| self.ui.nodes[i].rect)
                });
            if let Some(rect) = rect {
                self.ui.nodes[field].paint_offset = [
                    (rect.x + (rect.w - 160.) * 0.5).max(0.),
                    (rect.y - 3.).max(0.),
                ];
                self.ui.nodes[field].dirty.paint = true;
            }
            let id = ara_ui::source::stable_id("asset-rename-value");
            if self.input.focused() != Some(id) {
                self.input.focus(&mut self.ui, id)?;
                self.ui
                    .edit(field, ara_ui_text::EditEvent::SelectAll, &mut NoClipboard)?;
            }
        }
        Ok(())
    }

    pub(super) fn commit_asset_rename(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(rename) = self.asset_rename.clone() else {
            return Ok(false);
        };
        let folder = rename.path.rsplit_once('/').map_or("", |(p, _)| p);
        let name = rename.name.trim().trim_end_matches(".cs");
        let path = if folder.is_empty() {
            format!("{name}.cs")
        } else {
            format!("{folder}/{name}.cs")
        };
        let result=editor.action("script.rename",json!({"path":rename.path,"to":path,"attachTo":rename.attach_to,"doc":editor.state.doc}))?;
        self.browser.remove_path(&rename.path);
        self.browser.insert_path(&path, "cs");
        editor.action("editor.workspace", json!({"asset":path}))?;
        self.asset_rename = None;
        self.script_job = result["job"]
            .as_str()
            .map(|job| (job.to_owned(), std::time::Instant::now()));
        self.status_note = Some(format!("Compiling {name}.cs"));
        Ok(true)
    }

    pub(crate) fn open_script_ide(&self, editor: &Editor, path: &str) -> ara_ui::Result<()> {
        if !self.open_ide || self.offscreen {
            return Ok(());
        }
        let file = editor.host.root().join(path);
        let configured = editor
            .state
            .settings
            .get("scripts.ide")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !configured.is_empty() && configured != "system" {
            std::process::Command::new(configured)
                .arg(editor.host.root())
                .arg(&file)
                .spawn()?;
            return Ok(());
        }
        if configured == "system" {
            ops::open_external(&file)
        } else {
            open_in_ide(editor.host.root(), &file)
        }
    }

    pub(super) fn poll_script_job(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some((job, last)) = &mut self.script_job else {
            return Ok(false);
        };
        if last.elapsed() < std::time::Duration::from_millis(200) {
            return Ok(false);
        }
        *last = std::time::Instant::now();
        let result = editor.action("jobs.status", json!({"job":job}))?;
        let status = result["status"].as_str().unwrap_or("running");
        if status == "running" {
            return Ok(false);
        }
        self.status_note = Some(if status == "done" {
            format!(
                "{}.cs compiled",
                result["result"]["component"].as_str().unwrap_or("Script")
            )
        } else {
            result["diagnostics"][0]["message"]
                .as_str()
                .unwrap_or("Script compilation failed; see Problems")
                .to_owned()
        });
        self.script_job = None;
        Ok(true)
    }
}
