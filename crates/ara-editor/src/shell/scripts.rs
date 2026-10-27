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

/// Published C# module inputs from the shared D30 inventory; graph errors propagate.
pub(crate) fn modules(root: &Path) -> Result<Vec<PathBuf>, Vec<ara_diag::Diagnostic>> {
    ara_play_host::script_modules(root)
}

#[derive(Default)]
pub(super) struct ModuleList {
    paths: Vec<PathBuf>,
}
impl ModuleList {
    fn update(
        &mut self,
        result: Result<Vec<PathBuf>, Vec<ara_diag::Diagnostic>>,
    ) -> Vec<ara_diag::Diagnostic> {
        match result {
            Ok(paths) => {
                self.paths = paths;
                Vec::new()
            }
            Err(diagnostics) => diagnostics,
        }
    }
}

impl Shell {
    pub(crate) fn script_modules(&mut self, editor: &mut Editor) -> Vec<PathBuf> {
        let diagnostics = self.script_modules.update(modules(editor.host.root()));
        if !diagnostics.is_empty() {
            self.status_note = Some(
                "C# module discovery failed; showing the last good module list. See Problems."
                    .into(),
            );
            editor.report_failure(&crate::editor::EditorError(
                ara_proto::RpcError::from_diagnostics(diagnostics),
            ));
        }
        self.script_modules.paths.clone()
    }
}

#[cfg(test)]
mod module_tests {
    use super::*;
    #[test]
    fn failed_discovery_reports_diagnostics_and_keeps_the_last_good_module_list() {
        let mut modules = ModuleList::default();
        let good = vec![PathBuf::from(".ara/scripts/game")];
        assert!(modules.update(Ok(good.clone())).is_empty());
        let error = ara_diag::Diagnostic::error(
            ara_script::codes::UNKNOWN_MODULE,
            "combat references missing core",
        );
        let diagnostics = modules.update(Err(vec![error.clone()]));
        assert_eq!(diagnostics, vec![error]);
        assert_eq!(modules.paths, good);
        assert!(modules.update(Ok(vec![])).is_empty());
        assert!(
            modules.paths.is_empty(),
            "a successful empty inventory replaces the last good list"
        );
    }
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
    /// Reveal a diagnostic's exact source coordinates in the configured editor.
    pub(crate) fn open_script_location(
        &mut self,
        editor: &Editor,
        path: &str,
        line: Option<u32>,
        column: Option<u32>,
    ) -> ara_ui::Result<()> {
        let path = Path::new(path);
        let file = if path.is_absolute() {
            std::fs::canonicalize(path)?
        } else {
            crate::commands::project_path(editor.host.root(), &path.to_string_lossy())?
        };
        let root = std::fs::canonicalize(editor.host.root())?;
        if !file.starts_with(&root) {
            return Err("This diagnostic points outside the open project".into());
        }
        let line = line.unwrap_or(1).max(1);
        let column = column.unwrap_or(1).max(1);
        self.status_note = Some(format!("{}:{line}:{column}", file.display()));
        if !self.open_ide || self.offscreen {
            return Ok(());
        }
        let configured = editor
            .state
            .settings
            .get("scripts.ide")
            .and_then(Value::as_str)
            .unwrap_or("");
        if configured == "system" {
            return ops::open_external(&file);
        }
        let code = if configured.is_empty() {
            if cfg!(windows) { "code.cmd" } else { "code" }
        } else {
            configured
        };
        let mut command = std::process::Command::new(code);
        if configured.is_empty()
            || Path::new(code).file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with("code")
            })
        {
            command
                .arg(&root)
                .arg("--goto")
                .arg(format!("{}:{line}:{column}", file.display()));
        } else {
            command.arg(&file);
        }
        if command.spawn().is_err() {
            ops::open_external(&file)?;
        }
        Ok(())
    }

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
            commands::Target::Asset(p)
                if self
                    .browser
                    .items
                    .iter()
                    .any(|asset| asset.path == *p && asset.kind == "folder") =>
            {
                p.clone()
            }
            commands::Target::Asset(p) => p.rsplit_once('/').map_or("", |(p, _)| p).to_owned(),
            _ => editor.state.asset_folder.clone(),
        };
        let logical_folder = self.browser.authored_folder_ref(&folder)?;
        let folder = ops::authored::storage(
            &self.browser.files,
            editor.host.store().workspace(),
            logical_folder,
        )?;
        let stem = if template == "system" {
            "NewSystem"
        } else {
            "NewBehaviour"
        };
        let path = (1..=10_000)
            .map(|n| {
                let name = if n == 1 {
                    stem.to_owned()
                } else {
                    format!("{stem}{n}")
                };
                if logical_folder.is_empty() {
                    format!("{name}.cs")
                } else {
                    format!("{logical_folder}/{name}.cs")
                }
            })
            .find(|p| {
                ops::authored::available(&self.browser.files, editor.host.store().workspace(), p)
                    && !self
                        .browser
                        .items
                        .iter()
                        .any(|a| a.kind == "cs" && names::file_name(&a.path) == names::file_name(p))
            })
            .ok_or("No unused script name")?;
        // ScriptService emits project-relative source file operations. Its path is
        // physical storage, while document/reference commands use logical paths.
        let path =
            ops::authored::storage(&self.browser.files, editor.host.store().workspace(), &path)?;
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
