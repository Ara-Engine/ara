//! Human file gestures lower to one authority-owned typed command batch. The
//! browser never renames, copies or removes files, and retains selection until success.
use super::*;
use ara_ui::dialogs::{DialogFuture, DialogOptions};
use std::time::{Duration, Instant};
mod backend;
mod history;
mod jobs;
mod plan;
mod view;

#[derive(Clone, PartialEq)]
pub(crate) enum Action {
    RenameValue,
}
struct Rename {
    asset: assets::Asset,
    name: String,
    error: Option<String>,
    focus_requested: bool,
}
enum Prompt {
    Folder {
        parent: String,
        response: DialogFuture<Option<String>>,
    },
    Move {
        sources: Vec<assets::Asset>,
        response: DialogFuture<Option<String>>,
    },
}
struct Confirmation {
    plan: plan::Plan,
    response: DialogFuture<bool>,
}
struct Job {
    id: String,
    status: Value,
    polled: Instant,
    began: Instant,
    plan: plan::Plan,
    selection: Selection,
    following: bool,
    cancel_requested: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Selection {
    doc: String,
    entities: Vec<String>,
    asset: Option<String>,
    folder: String,
    keys: Vec<u64>,
    paths: Vec<String>,
    primary: Option<String>,
    active: Option<u64>,
}
impl Selection {
    fn read(shell: &Shell, editor: &Editor) -> Self {
        let mut keys: Vec<_> = shell.asset_selection.selected().iter().copied().collect();
        keys.sort_unstable();
        let mut paths: std::collections::BTreeSet<_> = shell
            .browser
            .items
            .iter()
            .chain(shell.asset_entries.values())
            .filter(|asset| {
                keys.binary_search(&ara_ui::source::stable_id(&asset.path))
                    .is_ok()
            })
            .map(|asset| asset.path.clone())
            .collect();
        if let Some(path) = &editor.state.asset_path {
            paths.insert(path.clone());
        }
        Self {
            doc: editor.state.doc.clone(),
            entities: editor.state.selection.clone(),
            asset: editor.state.asset_path.clone(),
            folder: editor.state.asset_folder.clone(),
            keys,
            paths: paths.into_iter().collect(),
            primary: shell.asset_selection_path.clone(),
            active: shell.asset_selection.active().copied(),
        }
    }
    fn unchanged(&self, shell: &Shell, editor: &Editor) -> bool {
        self.doc == editor.state.doc
            && self.entities == editor.state.selection
            && self.asset == editor.state.asset_path
            && self.folder == editor.state.asset_folder
            && self.primary == shell.asset_selection_path
            && self.keys.len() == shell.asset_selection.selected().len()
            && self
                .keys
                .iter()
                .all(|key| shell.asset_selection.selected().contains(key))
    }
    fn same_navigation(&self, other: &Self) -> bool {
        self.doc == other.doc
            && self.entities == other.entities
            && self.asset == other.asset
            && self.folder == other.folder
            && self.primary == other.primary
            && self.keys == other.keys
    }
}
/// Ephemeral dialog, draft and job state. Only the host owns file changes.
#[derive(Default)]
pub(crate) struct State {
    rename: Option<Rename>,
    prompt: Option<Prompt>,
    confirmation: Option<Confirmation>,
    job: Option<Job>,
    alert: Option<DialogFuture<()>>,
    history: history::History,
    pub revision: u64,
}
impl State {
    fn busy(&self) -> bool {
        self.prompt.is_some()
            || self.confirmation.is_some()
            || self.history.pending()
            || self
                .job
                .as_ref()
                .is_some_and(|job| backend::running(&job.status) || job.following)
    }
    pub(super) fn rename_path(&self) -> Option<&str> {
        self.rename
            .as_ref()
            .map(|rename| rename.asset.path.as_str())
    }
    /// A scan may observe committed files just before its job acknowledgement.
    /// Keep the human's keys alive until that acknowledgement and index handoff.
    pub(super) fn preserved_keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.job
            .iter()
            .filter(|job| backend::running(&job.status) || job.following)
            .flat_map(|job| job.selection.keys.iter().copied())
            .chain(self.history.preserved_keys())
    }
}
impl Shell {
    fn asset_ops_idle(&self) -> ara_ui::Result<()> {
        if self.asset_ops.busy() {
            return Err("Wait for the current asset operation, or cancel its preparation".into());
        }
        if self.asset_ops.rename.is_some() {
            return Err("Finish or cancel the inline asset name first".into());
        }
        Ok(())
    }
    fn asset_sources(
        &self,
        editor: &Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<Vec<assets::Asset>> {
        let path = self
            .asset_target(editor, target)
            .ok_or("Select an authored asset or folder first")?;
        let selected = self.asset_selection.selected();
        let use_selection = selected.contains(&ara_ui::source::stable_id(&path));
        let mut sources: Vec<_> = self
            .browser
            .items
            .iter()
            .filter(|asset| {
                if use_selection {
                    selected.contains(&ara_ui::source::stable_id(&asset.path))
                } else {
                    asset.path == path
                }
            })
            .cloned()
            .collect();
        if sources.is_empty() {
            return Err("The selected asset is not in the current authored index; wait for the browser to refresh".into());
        }
        sources.sort_by(|a, b| a.path.cmp(&b.path));
        for source in &sources {
            plan::source(source)?;
        }
        let mut roots: Vec<assets::Asset> = Vec::new();
        for source in sources {
            if !roots.iter().any(|root| {
                root.kind == "folder"
                    && source
                        .path
                        .strip_prefix(&root.path)
                        .is_some_and(|tail| tail.starts_with('/'))
            }) {
                roots.push(source);
            }
        }
        Ok(roots)
    }
    pub(crate) fn begin_asset_file_rename(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        self.asset_ops_idle()?;
        let path = self
            .asset_target(editor, target)
            .ok_or("Select an asset to rename")?;
        let asset = self
            .browser
            .items
            .iter()
            .find(|asset| asset.path == path)
            .ok_or("Wait for the authored asset index to refresh")?
            .clone();
        plan::source(&asset)?;
        self.asset_ops.rename = Some(Rename {
            name: asset.name().into(),
            asset,
            error: None,
            focus_requested: true,
        });
        self.asset_ops.revision += 1;
        self.menu = None;
        editor.action("editor.layout.show", json!({"panel":"assets"}))?;
        self.rebuild(editor)
    }
    pub(crate) fn cancel_asset_file_name(&mut self) -> bool {
        if self.asset_ops.rename.take().is_none() {
            return false;
        }
        self.input.blur(&mut self.ui);
        self.asset_ops.revision += 1;
        true
    }
    pub(crate) fn commit_asset_file_name(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(rename) = &self.asset_ops.rename else {
            return Ok(false);
        };
        let name = rename.name.trim();
        plan::name(name)?;
        if name == rename.asset.name() {
            self.cancel_asset_file_name();
            return Ok(true);
        }
        let source = plan::source(&rename.asset)?.to_owned();
        let destination = plan::join(plan::parent(&source), name);
        let plan = plan::Plan::new(
            "Rename asset",
            vec![json!({"cmd":"asset.rename","args":{"source":source,"name":name}})],
            vec![format!("Rename {}", rename.asset.path)],
            vec![(rename.asset.clone(), Some(destination))],
            false,
        );
        match self.start_asset_operation(editor, plan) {
            Ok(()) => {
                self.asset_ops.rename = None;
                self.input.blur(&mut self.ui);
                Ok(true)
            }
            Err(error) => {
                if let Some(rename) = &mut self.asset_ops.rename {
                    rename.error = Some(error.to_string());
                }
                Err(error)
            }
        }
    }
    pub(crate) fn duplicate_asset_files(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        self.asset_ops_idle()?;
        let sources = self.asset_sources(editor, target)?;
        let plan = plan::duplicate(sources, &self.browser.items)?;
        self.start_asset_operation(editor, plan)
    }
    pub(crate) fn request_delete_asset_files(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        self.asset_ops_idle()?;
        let sources = self.asset_sources(editor, target)?;
        let plan = plan::delete(sources)?;
        let mut options = DialogOptions::new(
            "Delete selected assets?",
            format!(
                "{}\n\nFolders include their contents. References will be checked and unsafe deletion blocked. One Undo restores deleted assets.",
                plan.labels.join("\n")
            ),
        );
        options.accept_label = "Delete".into();
        options.destructive = true;
        let response = self.confirmations.confirm(options);
        self.asset_ops.confirmation = Some(Confirmation { plan, response });
        self.asset_ops.revision += 1;
        self.menu = None;
        self.rebuild(editor)
    }
    pub(crate) fn request_move_asset_files(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        self.asset_ops_idle()?;
        let sources = self.asset_sources(editor, target)?;
        let destination = self.browser.authored_folder(&editor.state.asset_folder)?;
        let options = DialogOptions::new(
            "Move selected assets",
            "Choose an existing authored destination folder. Leave blank for the authored content root. References update in the same transaction.",
        );
        let response = self.confirmations.prompt(options, destination);
        self.asset_ops.prompt = Some(Prompt::Move { sources, response });
        self.asset_ops.revision += 1;
        self.menu = None;
        self.rebuild(editor)
    }
    pub(crate) fn move_asset_files_to(
        &mut self,
        editor: &mut Editor,
        source: &assets::Asset,
        storage_folder: &str,
    ) -> ara_ui::Result<()> {
        self.asset_ops_idle()?;
        let sources = self.asset_sources(editor, &commands::Target::Asset(source.path.clone()))?;
        let destination = self.browser.authored_folder(storage_folder)?;
        self.start_asset_operation(editor, plan::move_to(sources, &destination)?)
    }
    /// Read-only hover validation borrows cached rows; it does not clone a selected
    /// asset set or scan the filesystem on pointer movement.
    pub(crate) fn asset_move_preflight(
        &self,
        source: &assets::Asset,
        storage_folder: &str,
    ) -> Result<(), String> {
        if self.asset_ops.busy() || self.asset_ops.rename.is_some() {
            return Err("Finish the current asset operation before moving files".into());
        }
        let folder = self.browser.authored_folder_ref(storage_folder)?;
        if !self
            .browser
            .items
            .iter()
            .any(|asset| asset.path == source.path)
        {
            return Err("The dragged asset is no longer in the authored index".into());
        }
        let selected = self.asset_selection.selected();
        let use_selection = selected.contains(&ara_ui::source::stable_id(&source.path));
        for asset in self.browser.items.iter().filter(|asset| {
            if use_selection {
                selected.contains(&ara_ui::source::stable_id(&asset.path))
            } else {
                asset.path == source.path
            }
        }) {
            let path = plan::source(asset).map_err(|error| error.to_string())?;
            if asset.kind == "folder"
                && (folder == path
                    || folder
                        .strip_prefix(path)
                        .is_some_and(|tail| tail.starts_with('/')))
            {
                return Err(
                    "A folder cannot be moved into itself or one of its descendants".into(),
                );
            }
            if plan::parent(path) == folder {
                return Err("The selected asset is already in this folder".into());
            }
        }
        plan::source(source).map_err(|error| error.to_string())?;
        Ok(())
    }
    pub(crate) fn create_asset_folder(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        self.asset_ops_idle()?;
        let storage = match target {
            commands::Target::Folder(path) => path.as_str(),
            _ => editor.state.asset_folder.as_str(),
        };
        let parent = self.browser.authored_folder(storage)?;
        let options = DialogOptions::new(
            "Create folder",
            "Name the new folder inside the current authored folder.",
        );
        let response = self.confirmations.prompt(options, "New Folder".into());
        self.asset_ops.prompt = Some(Prompt::Folder { parent, response });
        self.asset_ops.revision += 1;
        self.menu = None;
        self.rebuild(editor)
    }
    pub(crate) fn asset_file_action(
        &mut self,
        _editor: &mut Editor,
        _action: &Action,
        value: Value,
    ) -> ara_ui::Result<()> {
        if let Some(rename) = &mut self.asset_ops.rename {
            rename.name = value.as_str().unwrap_or_default().into();
            rename.error = None;
        }
        self.asset_ops.revision += 1;
        Ok(())
    }
}
