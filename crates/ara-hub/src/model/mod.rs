//! The Hub view-model: everything the views render, changed only by user
//! [`Intent`]s and connection [`Event`]s. It owns no window, GPU or socket: it
//! queues [`Call`]s for the connection and [`Effect`]s for the native shell, so
//! the window and the headless tests drive exactly the same state machine.
mod call;
mod create;
pub mod engines;
mod notifications;
pub mod text;
pub mod updates;
mod wire;

pub use call::Call;
pub use create::CreateForm;
pub use wire::*;

use crate::connection::Event;
use ara_diag::{Diagnostic, FixAction, codes};
use ara_proto::RpcError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

/// Hub pages; project creation is a dedicated page outside the sidebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum View {
    /// Registered projects.
    #[default]
    Projects,
    /// New project from a template.
    Create,
    /// Installed engine builds and project pins.
    Installs,
    /// Agent onboarding and live activity.
    Agents,
    /// Samples and docs.
    Learn,
    /// Release channel, feed and Hub update controls.
    Settings,
}
impl View {
    /// All pages, including the dedicated creation page, for captures and checks.
    pub const ALL: [View; 6] = [
        View::Projects,
        View::Create,
        View::Installs,
        View::Agents,
        View::Learn,
        View::Settings,
    ];
    /// Persistent sidebar destinations.
    pub const SIDEBAR: [View; 5] = [
        View::Projects,
        View::Installs,
        View::Agents,
        View::Learn,
        View::Settings,
    ];
    /// Sidebar label and page title.
    pub fn title(self) -> &'static str {
        match self {
            View::Projects => "Projects",
            View::Create => "New project",
            View::Installs => "Installs",
            View::Agents => "Agents",
            View::Learn => "Learn",
            View::Settings => "Settings",
        }
    }
    /// Lucide icon name.
    pub fn icon(self) -> &'static str {
        match self {
            View::Projects => "layout-grid",
            View::Create => "folder-plus",
            View::Installs => "hard-drive",
            View::Agents => "bot",
            View::Learn => "graduation-cap",
            View::Settings => "refresh-cw",
        }
    }
}

/// Project ordering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Sort {
    /// Most recently opened first; never-opened projects last.
    #[default]
    Recent,
    /// Alphabetical.
    Name,
}

/// Project presentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    /// Thumbnail cards.
    #[default]
    Grid,
    /// Dense rows.
    List,
}

/// Agent client in the onboarding panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentClient {
    /// Claude Code.
    #[default]
    Claude,
    /// OpenAI Codex CLI.
    Codex,
}
impl AgentClient {
    /// The `--client` value.
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// UI preferences persisted beside the registry (`hub-ui.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Light theme instead of dark.
    pub light: bool,
    /// Project ordering.
    pub sort: Sort,
    /// Cards or rows.
    pub layout: Layout,
    /// Agent client last shown.
    pub agent: AgentClient,
    /// Signed Hub release identity explicitly skipped by the user.
    pub skipped_update: Option<String>,
    /// Whether Hub update downloads require a separate action; restarting always does.
    pub hub_update_downloads: DownloadPolicy,
}

/// Update downloads are separate from the always-explicit restart decision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadPolicy {
    /// Show the release and wait for Update now.
    #[default]
    Ask,
    /// Download a new, unskipped release in the background, then wait for Restart.
    Background,
}

/// Connection state shown in the sidebar footer.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    /// Starting or reconnecting.
    Connecting,
    /// Subscribed to `hub.changed`.
    Online,
    /// Unreachable; the worker retries. `log` is `hub-start.log`.
    Offline {
        /// Why.
        message: String,
        /// Service startup log.
        log: PathBuf,
    },
}

/// A lifecycle or edit in flight for one project. Shown as progress, never as an
/// approval (D24: lifecycle calls just run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    /// `hub.open` in the editor.
    Opening,
    /// `hub.open` headless.
    Starting,
    /// `hub.close`.
    Closing,
    /// `hub.remove`.
    Removing,
    /// `hub.update`.
    Saving,
}
impl Busy {
    /// Progress label.
    pub fn label(self) -> &'static str {
        match self {
            Busy::Opening => "Opening…",
            Busy::Starting => "Starting…",
            Busy::Closing => "Closing…",
            Busy::Removing => "Removing…",
            Busy::Saving => "Saving…",
        }
    }
}

/// A notice: an RPC failure shown in a dialog, or an informational toast.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    /// Stable ID for dismissal.
    pub id: u64,
    /// Headline (`Couldn't open Outpost`).
    pub title: String,
    /// Standard RpcError diagnostics (message + fixes); empty for confirmations.
    pub diagnostics: Vec<Diagnostic>,
    /// Confirmation detail line.
    pub detail: Option<String>,
    /// A call that reverses a confirmation (re-register after Remove).
    pub undo: Option<Call>,
    /// A log the failure points to (`.ara/hub-process.log`).
    pub log: Option<PathBuf>,
    /// A failed operation that can be deliberately retried from its error dialog.
    pub retry: Option<Box<Intent>>,
}
impl Notice {
    /// Errors use a dialog; successful results use informational toasts.
    pub fn is_error(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

/// What a folder picker was opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Add an existing project.
    Register,
    /// Create-view destination.
    Destination,
    /// Installs: register an engine build.
    Engine,
}

/// Native work the window performs for the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Launch the external updater and exit only this GUI.
    ApplyUpdate {
        archive: PathBuf,
        manifest: String,
        signature: String,
    },
    /// Show a folder picker; the answer comes back as [`Intent::Picked`].
    Pick(Pick),
    /// Select a folder in Explorer.
    Reveal(PathBuf),
    /// Open a file or folder with its default application.
    Open(PathBuf),
    /// Put text on the clipboard.
    Copy(String),
}

/// Everything a person can do in the Hub.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// Open a focused dialog without starting its operation.
    Dialog(Dialog),
    /// Close a dialog and restore the previous focus.
    DismissDialog,
    /// Run the action explicitly confirmed in the current dialog.
    ConfirmDialog,
    /// Skip this exact Hub version/build until a newer release is published.
    SkipUpdate,
    /// Refresh signed release metadata in the background.
    CheckUpdates,
    /// Persist how Hub updates are downloaded.
    HubUpdateDownloads(DownloadPolicy),
    /// Expand or collapse technical release-source settings.
    ToggleAdvanced,
    /// Refresh release jobs after a progress notification.
    RefreshDownloads,
    /// Stable or preview releases, persisted by the service.
    ReleaseChannel(ara_update::Channel),
    /// Edit the repository setting before saving.
    ReleaseRepo(String),
    /// Save the repository override.
    SaveReleaseRepo,
    /// Install a published engine version.
    InstallEngine(String),
    /// Download and repair this engine while retaining project version pins.
    RepairEngine(String),
    /// Retry an uninstall job, including cleanup after the engine was unregistered.
    RetryUninstall(String),
    /// Cancel a release job.
    CancelDownload(String),
    /// Cancel a transfer request whose background job has not answered yet.
    CancelPending { kind: String, version: String },
    /// Download the offered Hub update.
    PrepareUpdate,
    /// Restart this Hub through the verified external updater.
    RestartUpdate,
    /// Install a project's missing pin and then open it.
    InstallAndOpen { project: String, headless: bool },
    /// Sidebar navigation.
    Navigate(View),
    /// Return from project creation to the page that opened it.
    Back,
    /// Dark/light.
    ToggleTheme,
    /// Project search text.
    Search(String),
    /// Project ordering.
    Sort(Sort),
    /// Cards or rows.
    Layout(Layout),
    /// Open (Some) or close (None) a project's overflow menu.
    Menu(Option<String>),
    /// Open in the editor.
    Open(String),
    /// Open as a background daemon.
    OpenHeadless(String),
    /// Close the project's process.
    Close(String),
    /// Start renaming a project inline.
    Rename(String),
    /// Edit the inline name.
    RenameDraft(String),
    /// Save the inline name.
    RenameCommit,
    /// Drop the inline name.
    RenameCancel,
    /// Unregister, keeping files.
    Remove(String),
    /// Show the project folder in Explorer.
    Reveal(String),
    /// Folder picker for an existing project.
    AddExisting,
    /// Register a folder (picked or dropped on the window).
    Register(PathBuf),
    /// Folder picker result.
    Picked(Pick, PathBuf),
    /// Create view: choose a template.
    Template(String),
    /// Create view: project name.
    CreateName(String),
    /// Create view: destination folder.
    CreateDestination(String),
    /// Create view: browse for the destination.
    Browse,
    /// Create view: engine pin.
    CreateEngine(String),
    /// Create view: create it.
    CreateSubmit,
    /// Installs: pin a project to a version.
    Pin {
        /// Registry ID.
        project: String,
        /// Installed version.
        version: String,
    },
    /// Choose an installed engine and confirm its effect before changing the pin.
    ChooseEngine { project: String, version: String },
    /// Installs: register a build from a folder.
    AddEngine,
    /// Installs: remove the build at this index.
    RemoveEngine(usize),
    /// Remove the exact engine build confirmed by the user, even if the list refreshes.
    RemoveEngineBuild(Engine),
    /// Agents: pick the client shown.
    Agent(AgentClient),
    /// Copy text (commands, configs).
    Copy(String),
    /// Open a file, folder or URL with the shell.
    OpenPath(PathBuf),
    /// Dismiss a notice.
    Dismiss(u64),
    /// Hide this inline notification; new offline incidents/update versions reappear.
    DismissBanner(String),
    /// Run a notice's fix (a `hub.*` call) or its undo.
    Fix {
        /// Notice ID.
        notice: u64,
        /// Fix index across its diagnostics; `usize::MAX` is the undo.
        fix: usize,
    },
}

/// A dialog owns the decision, while background transfers continue independently.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    /// Release notes, download progress and the explicit restart step.
    Update,
    /// Confirm a published engine installation.
    InstallEngine(String),
    /// Confirm and track repair of an installed engine version.
    RepairEngine(String),
    /// Confirm deletion of this exact managed installation with its affected projects.
    UninstallEngine(Engine),
    /// Confirmation for an operation with a meaningful consequence.
    Confirm {
        /// Concise question naming the affected item.
        title: String,
        /// Consequence and recovery information.
        detail: String,
        /// The affirmative button label.
        label: String,
        /// Destructive actions receive the destructive treatment.
        destructive: bool,
        /// The command runs only after confirmation.
        action: Box<Intent>,
    },
    /// A catalogued error, with its fixes and optional retry.
    Error(u64),
}

/// Lists refreshed from the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum List {
    Projects,
    Status,
    Templates,
    Engines,
    Activity,
}
impl List {
    const ALL: [List; 5] = [
        List::Projects,
        List::Status,
        List::Templates,
        List::Engines,
        List::Activity,
    ];
    fn call(self) -> Call {
        match self {
            List::Projects => Call::Projects,
            List::Status => Call::Status,
            List::Templates => Call::Templates,
            List::Engines => Call::Engines,
            List::Activity => Call::Activity,
        }
    }
    fn noun(self) -> &'static str {
        match self {
            List::Projects => "projects",
            List::Status => "project status",
            List::Templates => "templates",
            List::Engines => "engine builds",
            List::Activity => "agent activity",
        }
    }
    fn of(call: &Call) -> Option<Self> {
        Self::ALL.into_iter().find(|list| list.call() == *call)
    }
}

/// The Hub's whole UI state.
pub struct Hub {
    /// Technical release-source controls are explicitly disclosed.
    pub advanced: bool,
    /// Current input-trapping dialog.
    pub dialog: Option<Dialog>,
    /// Available versions, transfer jobs and update offer.
    pub releases: updates::Releases,
    /// Current page.
    pub view: View,
    /// Persisted preferences.
    pub prefs: Prefs,
    /// Service connection.
    pub link: Link,
    /// Registered projects, registry order.
    pub projects: Vec<Project>,
    /// Status per project ID.
    pub status: BTreeMap<String, Status>,
    /// Create-view templates.
    pub templates: Vec<Template>,
    /// Installed engine builds.
    pub engines: Vec<Engine>,
    /// Participants per open project.
    pub activity: Vec<Activity>,
    /// Project search.
    pub search: String,
    /// Project whose overflow menu is open.
    pub menu: Option<String>,
    /// Inline rename: project ID and draft.
    pub rename: Option<(String, String)>,
    /// Work in flight per project ID.
    pub busy: BTreeMap<String, Busy>,
    /// Most recently added or created project, highlighted once.
    pub highlight: Option<String>,
    /// The Create form.
    pub create: CreateForm,
    /// Toasts, oldest first.
    pub notices: Vec<Notice>,
    /// Per-engine removal refusals, by version.
    pub engine_errors: BTreeMap<String, String>,
    /// `ara hub install-mcp` output per client (filled by the native shell).
    pub install: BTreeMap<AgentClient, String>,
    /// Verified absolute CLI path used by the displayed MCP setup commands.
    pub cli_executable: Option<PathBuf>,
    /// The installed build this Hub ships with; registered on first run.
    pub local_engine: Option<Engine>,
    /// Field the window should focus after the next rebuild.
    pub focus: Option<&'static str>,
    /// Text last copied, so its button reads "Copied" until the next navigation.
    pub copied: Option<String>,
    loaded: BTreeSet<List>,
    inflight: BTreeSet<List>,
    stale: BTreeSet<List>,
    calls: Vec<Call>,
    effects: Vec<Effect>,
    next_notice: u64,
    registered_local: bool,
    notice_times: BTreeMap<u64, std::time::Instant>,
    dismissed_banners: BTreeSet<String>,
    create_origin: View,
}

impl Hub {
    /// A disconnected model. `home` seeds the Create destination.
    pub fn new(prefs: Prefs, home: PathBuf, local_engine: Option<Engine>) -> Self {
        Self {
            advanced: false,
            dialog: None,
            releases: updates::Releases::default(),
            view: View::Projects,
            prefs,
            link: Link::Connecting,
            projects: Vec::new(),
            status: BTreeMap::new(),
            templates: Vec::new(),
            engines: Vec::new(),
            activity: Vec::new(),
            search: String::new(),
            menu: None,
            rename: None,
            busy: BTreeMap::new(),
            highlight: None,
            create: CreateForm::new(home),
            notices: Vec::new(),
            engine_errors: BTreeMap::new(),
            install: BTreeMap::new(),
            cli_executable: None,
            local_engine,
            focus: None,
            copied: None,
            loaded: BTreeSet::new(),
            inflight: BTreeSet::new(),
            stale: BTreeSet::new(),
            calls: Vec::new(),
            effects: Vec::new(),
            next_notice: 1,
            registered_local: false,
            notice_times: BTreeMap::new(),
            dismissed_banners: BTreeSet::new(),
            create_origin: View::Projects,
        }
    }

    /// Calls for the connection, in order.
    pub fn take_calls(&mut self) -> Vec<Call> {
        std::mem::take(&mut self.calls)
    }
    /// Native work for the window.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }
    /// True once every list has loaded at least once.
    pub fn ready(&self) -> bool {
        self.loaded.len() == List::ALL.len()
    }
    /// True while any refresh or action is in flight.
    pub fn idle(&self) -> bool {
        self.inflight.is_empty()
            && self.calls.is_empty()
            && self.busy.is_empty()
            && !self.create.waiting()
    }

    /// Projects as shown: search-filtered and sorted.
    pub fn visible(&self) -> Vec<&Project> {
        let needle = self.search.trim().to_lowercase();
        let mut list: Vec<_> = self
            .projects
            .iter()
            .filter(|p| {
                needle.is_empty()
                    || p.name.to_lowercase().contains(&needle)
                    || text::path(&p.path).to_lowercase().contains(&needle)
            })
            .collect();
        match self.prefs.sort {
            Sort::Recent => list.sort_by_key(|p| std::cmp::Reverse(p.last_opened.unwrap_or(0))),
            Sort::Name => list.sort_by_key(|p| p.name.to_lowercase()),
        }
        list
    }
    /// A project's badge state.
    pub fn state(&self, project: &str) -> State {
        self.status
            .get(project)
            .map_or(State::Closed, Status::state)
    }
    /// Project by ID.
    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    fn refresh(&mut self, lists: impl IntoIterator<Item = List>) {
        for list in lists {
            if self.inflight.insert(list) {
                self.calls.push(list.call());
            } else {
                self.stale.insert(list);
            }
        }
    }
    fn send(&mut self, call: Call) {
        self.calls.push(call);
    }
    fn notice(&mut self, title: String, error: Option<&RpcError>) -> u64 {
        let id = self.next_notice;
        self.next_notice += 1;
        // A repeated failure replaces its previous toast instead of stacking.
        self.notices.retain(|n| n.title != title);
        self.notices.push(Notice {
            id,
            title,
            diagnostics: error.map(RpcError::diagnostics).unwrap_or_default(),
            detail: None,
            undo: None,
            log: None,
            retry: None,
        });
        self.notice_times.insert(id, std::time::Instant::now());
        if error.is_some() {
            self.menu = None;
            // A background reply must not replace a decision while it is being made.
            if !matches!(
                self.dialog,
                Some(Dialog::Confirm { .. } | Dialog::UninstallEngine(_))
            ) {
                self.dialog = Some(Dialog::Error(id));
            }
        }
        if self.notices.len() > 4 {
            self.notices.remove(0);
        }
        id
    }
    fn name_of(&self, id: &str) -> String {
        self.project(id)
            .map_or_else(|| id.to_owned(), |p| p.name.clone())
    }

    /// Apply one user intent.
    pub fn intent(&mut self, intent: Intent) {
        match intent {
            Intent::Dialog(dialog) => {
                match self.dialog.take() {
                    Some(Dialog::Error(id)) => self.notices.retain(|notice| notice.id != id),
                    Some(Dialog::UninstallEngine(engine)) if !matches!(&dialog, Dialog::UninstallEngine(next) if next.version == engine.version) => {
                        self.close_removal(&engine.version)
                    }
                    _ => {}
                }
                self.menu = None;
                if let Dialog::InstallEngine(version) | Dialog::RepairEngine(version) = &dialog {
                    self.preflight_engine(version.clone());
                }
                if let Dialog::UninstallEngine(engine) = &dialog {
                    self.inspect_removal(engine.version.clone());
                }
                self.dialog = Some(dialog);
            }
            Intent::DismissDialog => match self.dialog.take() {
                Some(Dialog::Error(id)) => self.notices.retain(|notice| notice.id != id),
                Some(Dialog::UninstallEngine(engine)) => self.close_removal(&engine.version),
                _ => {}
            },
            Intent::ConfirmDialog => match self.dialog.take() {
                Some(Dialog::Confirm { action, .. }) => self.intent(*action),
                Some(Dialog::UninstallEngine(engine))
                    if self.releases.removal(&engine.version).is_some() =>
                {
                    self.intent(Intent::RemoveEngineBuild(engine))
                }
                Some(dialog @ Dialog::UninstallEngine(_)) => self.dialog = Some(dialog),
                _ => {}
            },
            Intent::SkipUpdate => {
                self.prefs.skipped_update = self.releases.hub.as_ref().map(updates::identity);
                self.dialog = None;
            }
            Intent::CheckUpdates => self.check_updates(),
            Intent::HubUpdateDownloads(policy) => {
                self.prefs.hub_update_downloads = policy;
                self.prepare_background_update();
            }
            Intent::ToggleAdvanced => self.advanced = !self.advanced,
            Intent::RefreshDownloads => self.release_call("hub.downloads", serde_json::json!({})),
            Intent::ReleaseChannel(channel) => {
                self.releases.channel = channel;
                self.release_call(
                    "hub.releases.configure",
                    serde_json::json!({"channel":channel}),
                );
            }
            Intent::ReleaseRepo(repo) => self.releases.repo = repo,
            Intent::SaveReleaseRepo => self.release_call(
                "hub.releases.configure",
                serde_json::json!({"repo":self.releases.repo}),
            ),
            Intent::InstallEngine(version) => self.install_engine(version),
            Intent::RepairEngine(version) => {
                self.start_transfer("repair", version, "hub.engine.repair")
            }
            Intent::CancelPending { kind, version } => self.cancel_pending(kind, version),
            Intent::RetryUninstall(version) => {
                if let Some(Dialog::Error(id)) = self.dialog.take() {
                    self.notices.retain(|notice| notice.id != id);
                }
                self.start_transfer("uninstall", version, "hub.engine.uninstall");
            }
            Intent::CancelDownload(job) => {
                if let Some(version) = self
                    .releases
                    .jobs
                    .iter()
                    .find(|entry| entry.id == job && entry.kind == "engine")
                    .map(|entry| entry.version.clone())
                {
                    self.releases.open_after.remove(&version);
                }
                self.release_call("hub.download.cancel", serde_json::json!({"job":job}))
            }
            Intent::PrepareUpdate => {
                if let Some(release) = &self.releases.hub {
                    self.start_transfer("hub", release.version.clone(), "hub.update.prepare");
                }
            }
            Intent::RestartUpdate => self.restart_update(),
            Intent::InstallAndOpen { project, headless } => {
                if let Some(p) = self.project(&project) {
                    let version = p.engine_version.clone();
                    self.releases
                        .open_after
                        .entry(version.clone())
                        .or_default()
                        .insert(project, headless);
                    self.install_engine(version);
                }
            }
            Intent::Navigate(view) => {
                if view == View::Create && self.view != View::Create {
                    self.create_origin = self.view;
                    self.focus = Some("create-name");
                }
                self.view = view;
                self.menu = None;
                self.copied = None;
                if view == View::Create {
                    self.create
                        .validate(&self.projects, &self.engines, &self.templates);
                    self.calls.extend(self.create.check());
                }
            }
            Intent::Back => {
                if self.view == View::Create {
                    self.view = self.create_origin;
                    self.menu = None;
                    self.copied = None;
                    self.focus = None;
                }
            }
            Intent::ToggleTheme => self.prefs.light = !self.prefs.light,
            Intent::Search(text) => self.search = text,
            Intent::Sort(sort) => self.prefs.sort = sort,
            Intent::Layout(layout) => self.prefs.layout = layout,
            Intent::Menu(project) => self.menu = project,
            Intent::Open(project) => self.lifecycle(project, Busy::Opening),
            Intent::OpenHeadless(project) => self.lifecycle(project, Busy::Starting),
            Intent::Close(project) => self.lifecycle(project, Busy::Closing),
            Intent::Remove(project) => self.lifecycle(project, Busy::Removing),
            Intent::Rename(project) => {
                self.menu = None;
                let name = self.name_of(&project);
                self.rename = Some((project, name));
                self.focus = Some("rename-field");
            }
            Intent::RenameDraft(text) => {
                if let Some((_, draft)) = &mut self.rename {
                    *draft = text;
                }
            }
            Intent::RenameCommit => {
                if let Some((project, name)) = self.rename.take() {
                    let name = name.trim().to_owned();
                    if !name.is_empty() && name != self.name_of(&project) {
                        self.busy.insert(project.clone(), Busy::Saving);
                        self.send(Call::Rename { project, name });
                    }
                }
            }
            Intent::RenameCancel => self.rename = None,
            Intent::Reveal(project) => {
                self.menu = None;
                if let Some(p) = self.project(&project) {
                    self.effects.push(Effect::Reveal(p.path.clone()));
                }
            }
            Intent::AddExisting => self.effects.push(Effect::Pick(Pick::Register)),
            Intent::Register(path) | Intent::Picked(Pick::Register, path) => {
                // A dropped `ara.project.json` (or any file) means its folder.
                let path = if path.is_file() {
                    path.parent().map_or(path.clone(), PathBuf::from)
                } else {
                    path
                };
                self.send(Call::Register { path });
            }
            Intent::Picked(Pick::Destination, path) => {
                self.create.destination = text::path(&path);
                self.create_changed();
            }
            Intent::Picked(Pick::Engine, folder) => match engines::probe(&folder) {
                Ok(engine) => self.send(Call::EngineRegister(engine)),
                Err(message) => {
                    let error = Diagnostic::error(codes::INVALID_PARAMS, message).into();
                    self.notice("Couldn't add the engine".into(), Some(&error));
                }
            },
            Intent::Template(id) => {
                if self.view != View::Create {
                    self.create_origin = self.view;
                    self.focus = Some("create-name");
                }
                if let Some(version) = self
                    .templates
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.engine_version.clone())
                {
                    self.create.engine = Some(version);
                }
                self.create.template = Some(id);
                self.view = View::Create;
                self.create_changed();
            }
            Intent::CreateName(name) => {
                self.create.name = name;
                self.create.named = true;
                self.create_changed();
            }
            Intent::CreateDestination(path) => {
                self.create.destination = path;
                self.create_changed();
            }
            Intent::Browse => self.effects.push(Effect::Pick(Pick::Destination)),
            Intent::CreateEngine(version) => {
                self.create.engine = Some(version);
                self.create_changed();
            }
            Intent::CreateSubmit => {
                if let Some(call) = self.create.submit() {
                    self.send(call);
                }
            }
            Intent::ChooseEngine { project, version } => {
                if self
                    .project(&project)
                    .is_some_and(|entry| entry.engine_version != version)
                {
                    self.intent(Intent::Dialog(Dialog::Confirm {
                        title: format!("Switch {} to Ara {version}?", self.name_of(&project)),
                        detail: "The project will use this editor version the next time it opens. Check your project's version control before opening it in a newer engine.".into(),
                        label: "Switch engine".into(), destructive: false,
                        action: Box::new(Intent::Pin { project, version }),
                    }));
                }
            }
            Intent::Pin { project, version } => {
                let changed = self
                    .project(&project)
                    .is_some_and(|p| p.engine_version != version);
                if changed
                    && self.state(&project) == State::Closed
                    && !self.busy.contains_key(&project)
                {
                    self.busy.insert(project.clone(), Busy::Saving);
                    self.send(Call::Pin { project, version });
                }
            }
            Intent::AddEngine => self.effects.push(Effect::Pick(Pick::Engine)),
            Intent::RemoveEngine(index) => {
                if let Some(engine) = self.engines.get(index).cloned() {
                    self.intent(Intent::RemoveEngineBuild(engine));
                }
            }
            Intent::RemoveEngineBuild(engine) => {
                self.dismissed_banners
                    .remove(&format!("engine-error-{}", engine.version));
                self.engine_errors.remove(&engine.version);
                if engine.managed {
                    self.start_transfer("uninstall", engine.version, "hub.engine.uninstall");
                } else {
                    self.send(Call::EngineRemove(engine));
                }
            }
            Intent::Agent(client) => self.prefs.agent = client,
            Intent::Copy(text) => {
                self.effects.push(Effect::Copy(text.clone()));
                self.copied = Some(text);
            }
            Intent::OpenPath(path) => self.effects.push(Effect::Open(path)),
            Intent::Dismiss(id) => {
                self.notices.retain(|n| n.id != id);
                if self.dialog == Some(Dialog::Error(id)) {
                    self.dialog = None;
                }
            }
            Intent::DismissBanner(key) => {
                self.dismissed_banners.insert(key);
            }
            Intent::Fix { notice, fix } => {
                self.fix(notice, fix);
                if !self.notices.iter().any(|entry| entry.id == notice)
                    && self.dialog == Some(Dialog::Error(notice))
                {
                    self.dialog = None;
                }
            }
        }
        if self.dialog.is_none()
            && let Some(notice) = self.notices.iter().rev().find(|notice| notice.is_error())
        {
            self.dialog = Some(Dialog::Error(notice.id));
        }
    }

    fn lifecycle(&mut self, project: String, busy: Busy) {
        self.menu = None;
        if self.busy.contains_key(&project) {
            return;
        }
        let call = match busy {
            Busy::Opening | Busy::Starting => Call::Open {
                project: project.clone(),
                headless: busy == Busy::Starting,
            },
            Busy::Closing => Call::Close {
                project: project.clone(),
            },
            Busy::Removing => Call::Remove {
                project: project.clone(),
            },
            Busy::Saving => return,
        };
        self.busy.insert(project, busy);
        self.send(call);
    }

    fn create_changed(&mut self) {
        self.create
            .validate(&self.projects, &self.engines, &self.templates);
        self.calls.extend(self.create.check());
    }

    fn fix(&mut self, notice: u64, index: usize) {
        let Some(found) = self.notices.iter().find(|n| n.id == notice).cloned() else {
            return;
        };
        if index == usize::MAX {
            if let Some(undo) = found.undo {
                self.notices.retain(|n| n.id != notice);
                self.send(undo);
            }
            return;
        }
        let fixes: Vec<_> = found.diagnostics.iter().flat_map(|d| &d.fixes).collect();
        match fixes.get(index).map(|f| &f.action) {
            Some(FixAction::Call { method, params }) if method.starts_with("hub.") => {
                // Fixes are diagnostics' own suggestions; only Hub methods run here.
                if let Some(call) = call_from(method, params) {
                    self.notices.retain(|n| n.id != notice);
                    self.send(call);
                }
            }
            Some(FixAction::Cli { args }) => {
                self.intent(Intent::Copy(format!("ara {}", args.join(" "))));
            }
            _ => {}
        }
    }

    /// Apply one connection event.
    pub fn event(&mut self, event: Event) {
        match event {
            Event::Prepared { executable } => self.cli_executable = Some(executable),
            Event::Connected => {
                self.dismissed_banners.remove("offline");
                self.link = Link::Online;
                // No replay is promised after (re)connecting: refresh every view.
                self.inflight.clear();
                self.stale.clear();
                self.refresh(List::ALL);
            }
            Event::ServiceUpdatePending => {
                let id = self.notice("Restart the Hub service".into(), None);
                if let Some(notice) = self.notices.iter_mut().find(|notice| notice.id == id) {
                    notice.detail = Some(
                        "Close open projects, then restart Ara Hub to finish updating.".into(),
                    );
                }
            }
            Event::Offline { message, log } => {
                if self.link
                    != (Link::Offline {
                        message: message.clone(),
                        log: log.clone(),
                    })
                {
                    self.dismissed_banners.remove("offline");
                }
                self.link = Link::Offline { message, log };
                self.inflight.clear();
                self.stale.clear();
            }
            Event::Changed { kind, .. } => match kind.as_str() {
                "releases" => self.refresh_releases(),
                "progress" => self.release_call("hub.downloads", serde_json::json!({})),
                "engine" => self.refresh([List::Engines, List::Templates]),
                "activity" => self.refresh([List::Status, List::Activity]),
                _ => self.refresh([List::Projects, List::Status, List::Activity]),
            },
            Event::Reply { call, result } => self.reply(call, result),
        }
    }

    fn reply(&mut self, call: Call, result: Result<Value, RpcError>) {
        if let Call::Release { method, params } = &call {
            if *method == "hub.download.cancel"
                && result.is_err()
                && params["job"]
                    .as_str()
                    .is_some_and(|job| self.removal_cancel(job))
            {
                return;
            }
            if *method == "hub.engine.preflight"
                && let Some(version) = params["version"].as_str()
            {
                self.preflight_reply(version.into(), result);
                return;
            }
            if *method == "hub.engine.uninstall.preflight"
                && let Err(error) = &result
            {
                if let Some(version) = params["version"].as_str() {
                    self.transfer_failed(method, version);
                    self.removal_request_failed(version, error);
                }
                return;
            }
            if result.is_err()
                && let Some(version) = params["version"].as_str()
            {
                self.transfer_failed(method, version);
            }
            let retry = result.as_ref().err().and_then(|_| match *method {
                "hub.engine.install" => params["version"]
                    .as_str()
                    .map(|version| Intent::Dialog(Dialog::InstallEngine(version.into()))),
                "hub.engine.repair" => params["version"]
                    .as_str()
                    .map(|version| Intent::Dialog(Dialog::RepairEngine(version.into()))),
                "hub.engine.uninstall" => params["version"]
                    .as_str()
                    .map(|version| Intent::RetryUninstall(version.into())),
                "hub.update.prepare" => Some(Intent::Dialog(Dialog::Update)),
                _ => None,
            });
            self.release_reply(method, result);
            if let Some(retry) = retry
                && let Some(notice) = self.notices.last_mut()
            {
                notice.retry = Some(Box::new(retry));
            }
            return;
        }
        if let Some(list) = List::of(&call) {
            self.inflight.remove(&list);
            match result {
                Ok(value) => self.loaded_list(list, &value),
                Err(error) => {
                    let title = format!("Couldn't refresh {}", list.noun());
                    self.notice(title, Some(&error));
                }
            }
            if self.stale.remove(&list) {
                self.refresh([list]);
            }
            return;
        }
        match call {
            Call::Release { .. } => unreachable!("handled above"),
            Call::Create { dry_run: true, .. } => {
                let next = self.create.checked(&call, result);
                self.calls.extend(next);
            }
            Call::Create { .. } => match self.create.created(result) {
                Ok(project) => {
                    let project_id = project.id.clone();
                    self.highlight = Some(project.id.clone());
                    self.search.clear();
                    self.view = View::Projects;
                    let id = self.notice(format!("Created {}", project.name), None);
                    self.detail(id, text::path(&project.path));
                    self.projects.push(project);
                    self.refresh([List::Projects, List::Status]);
                    self.lifecycle(project_id, Busy::Opening);
                }
                // Field problems show inline; anything else (IO, template copy) as a toast.
                Err(error) => {
                    if self.create.general().next().is_some() {
                        self.notice("Couldn't create the project".into(), Some(&error));
                    }
                }
            },
            Call::Open { project, headless } => {
                self.busy.remove(&project);
                match result {
                    Ok(value) => self.set_status(&value),
                    Err(error) => {
                        let what = if headless { "start" } else { "open" };
                        let title = format!("Couldn't {what} {}", self.name_of(&project));
                        let id = self.notice(title, Some(&error));
                        let log = self
                            .project(&project)
                            .map(|p| p.path.join(".ara/hub-process.log"));
                        if let Some(n) = self.notices.iter_mut().find(|n| n.id == id) {
                            n.log = log.filter(|path| path.is_file());
                        }
                    }
                }
            }
            Call::Close { project } => {
                self.busy.remove(&project);
                match result {
                    Ok(value) => self.set_status(&value),
                    Err(error) => {
                        let title = format!("Couldn't close {}", self.name_of(&project));
                        self.notice(title, Some(&error));
                    }
                }
            }
            Call::Remove { project } => {
                self.busy.remove(&project);
                match result {
                    Ok(_) => {
                        let removed = self.project(&project).cloned();
                        self.projects.retain(|p| p.id != project);
                        self.status.remove(&project);
                        if let Some(removed) = removed {
                            let id = self
                                .notice(format!("Removed {} from the list", removed.name), None);
                            self.detail(id, "Its files were kept.".into());
                            if let Some(n) = self.notices.iter_mut().find(|n| n.id == id) {
                                n.undo = Some(Call::Register { path: removed.path });
                            }
                        }
                    }
                    Err(error) => {
                        let title = format!("Couldn't remove {}", self.name_of(&project));
                        self.notice(title, Some(&error));
                    }
                }
            }
            Call::Rename { project, .. } | Call::Pin { project, .. } => {
                self.busy.remove(&project);
                match result {
                    Ok(value) => self.set_project(&value),
                    Err(error) => {
                        let title = format!("Couldn't update {}", self.name_of(&project));
                        self.notice(title, Some(&error));
                    }
                }
            }
            Call::Register { .. } => match result {
                Ok(value) => {
                    if let Ok(project) = serde_json::from_value::<Project>(value) {
                        self.highlight = Some(project.id.clone());
                        self.view = View::Projects;
                        self.search.clear();
                        match self.projects.iter_mut().find(|p| p.id == project.id) {
                            Some(slot) => *slot = project,
                            None => self.projects.push(project),
                        }
                    }
                }
                Err(error) => {
                    self.notice("Couldn't add the project".into(), Some(&error));
                }
            },
            Call::EngineRegister(_) => {
                if let Err(error) = result {
                    self.notice("Couldn't add the engine".into(), Some(&error));
                }
            }
            Call::EngineRemove(engine) => {
                if let Err(error) = result {
                    let reason = error
                        .diagnostics()
                        .first()
                        .map_or_else(|| error.message.clone(), |d| d.message.clone());
                    self.engine_errors.insert(engine.version, reason);
                    self.notice("Couldn't remove the engine".into(), Some(&error));
                }
            }
            Call::Projects | Call::Status | Call::Templates | Call::Engines | Call::Activity => {}
        }
    }

    fn detail(&mut self, id: u64, detail: String) {
        if let Some(n) = self.notices.iter_mut().find(|n| n.id == id) {
            n.detail = Some(detail);
        }
    }

    fn set_status(&mut self, value: &Value) {
        if let Ok(status) = serde_json::from_value::<Status>(value.clone()) {
            self.set_project(&value["project"]);
            self.status.insert(status.project.id.clone(), status);
        }
    }
    fn set_project(&mut self, value: &Value) {
        if let Ok(project) = serde_json::from_value::<Project>(value.clone())
            && let Some(slot) = self.projects.iter_mut().find(|p| p.id == project.id)
        {
            *slot = project;
        }
    }

    fn loaded_list(&mut self, list: List, value: &Value) {
        self.loaded.insert(list);
        let field = |key: &str| value.get(key).cloned().unwrap_or(Value::Null);
        match list {
            List::Projects => {
                self.projects = serde_json::from_value(field("projects")).unwrap_or_default();
                let ids: BTreeSet<_> = self.projects.iter().map(|p| p.id.clone()).collect();
                self.status.retain(|id, _| ids.contains(id));
                self.busy.retain(|id, _| ids.contains(id));
                if self
                    .rename
                    .as_ref()
                    .is_some_and(|(id, _)| !ids.contains(id))
                {
                    self.rename = None;
                }
            }
            List::Status => {
                let statuses: Vec<Status> =
                    serde_json::from_value(field("projects")).unwrap_or_default();
                self.status = statuses
                    .into_iter()
                    .map(|s| (s.project.id.clone(), s))
                    .collect();
            }
            List::Templates => {
                self.templates = serde_json::from_value(field("templates")).unwrap_or_default();
                self.create_changed();
            }
            List::Engines => {
                self.engines = serde_json::from_value(field("engines")).unwrap_or_default();
                let versions: BTreeSet<_> = self.engines.iter().map(|e| &e.version).collect();
                self.engine_errors.retain(|v, _| versions.contains(v));
                self.register_local();
                self.create_changed();
            }
            List::Activity => {
                self.activity = serde_json::from_value(field("activity")).unwrap_or_default();
            }
        }
    }

    /// First run of an installed build: make sure the Hub service knows it.
    fn register_local(&mut self) {
        if self.registered_local {
            return;
        }
        self.registered_local = true;
        if let Some(local) = self.local_engine.clone()
            && !self.engines.contains(&local)
        {
            self.send(Call::EngineRegister(local));
        }
    }
}

/// A notice fix's `hub.*` method as a typed call, when it is one the Hub knows.
fn call_from(method: &str, params: &Value) -> Option<Call> {
    let project = || params["project"].as_str().map(str::to_owned);
    match method {
        "hub.open" => Some(Call::Open {
            project: project()?,
            headless: params["headless"].as_bool().unwrap_or(false),
        }),
        "hub.close" => Some(Call::Close {
            project: project()?,
        }),
        "hub.register" => Some(Call::Register {
            path: params["path"].as_str()?.into(),
        }),
        _ => None,
    }
}
