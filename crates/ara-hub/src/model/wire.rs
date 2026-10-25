//! Hub service results as the UI reads them (`crates/ara-hub-service/AGENTS.md`).
//! Unknown fields are ignored so a newer service never breaks an older Hub.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// A registered project card from `hub.projects`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    /// Stable registry ID (`project-3`).
    pub id: String,
    /// Unique display name.
    pub name: String,
    /// Canonical absolute directory.
    pub path: PathBuf,
    /// Last successful open, Unix milliseconds.
    pub last_opened: Option<u64>,
    /// Preview image chosen by the UI or editor.
    pub thumbnail: Option<PathBuf>,
    /// Pinned engine version.
    pub engine_version: String,
}

/// Whether a project runs, and how, from `hub.status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// No authority process.
    #[default]
    Closed,
    /// Open in the native editor.
    Editor,
    /// Open as a background (headless) daemon.
    Headless,
}

/// One `hub.status` entry.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Status {
    /// The card, as of the status call.
    pub project: Project,
    /// `open` or `closed`.
    pub state: String,
    /// Authority process ID while open.
    pub pid: Option<u32>,
    /// Engine Protocol port while open.
    pub port: Option<u16>,
    /// True for a headless daemon; null while closed.
    pub headless: Option<bool>,
}
impl Status {
    /// The badge state; the service's `state`, never stale discovery files.
    pub fn state(&self) -> State {
        match (self.state.as_str(), self.headless) {
            ("open", Some(true)) => State::Headless,
            ("open", _) => State::Editor,
            _ => State::Closed,
        }
    }
}

/// A template tile from `hub.templates`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Template {
    /// Engine supplying the template, used as the new project's default pin.
    pub engine_version: Option<String>,
    /// Source-local ID passed to `hub.create`.
    pub id: String,
    /// Title.
    pub name: String,
    /// Preview image.
    pub thumbnail: Option<PathBuf>,
    /// One-line summary.
    pub description: Option<String>,
    /// Missing prerequisite (sample art not fetched).
    pub setup: Option<Setup>,
}

/// What a template still needs before `hub.create` accepts it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Setup {
    /// What is missing.
    pub message: String,
    /// The command that restores it.
    pub command: String,
}

/// An installed engine build from `hub.engines`; also the `engine` param of
/// `hub.engine.register` / `hub.engine.remove`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Engine {
    /// Hub owns the files and can uninstall them; external builds are unregister-only.
    #[serde(skip_serializing)]
    pub managed: bool,
    /// Version matched by project pins.
    pub version: String,
    /// Absolute `ara` CLI path.
    pub executable: PathBuf,
    /// Absolute `ara-editor` path, when the build has one.
    pub editor: Option<PathBuf>,
}

/// Participants of one open project, from `hub.activity`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Activity {
    /// Registry ID of the project.
    pub project: String,
    /// Everyone attached: editors, CLI humans and agents.
    pub participants: Vec<Participant>,
    /// Presence per participant ID.
    pub presence: BTreeMap<String, Presence>,
}

/// One attached session.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Participant {
    /// Session handle.
    pub id: String,
    /// `human` or `agent`.
    pub kind: String,
    /// Display name (`Mia`, `claude-code`).
    pub name: String,
    /// When it joined, host milliseconds.
    pub joined_ms: u64,
}

/// What a participant is looking at and doing.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Presence {
    /// Open document.
    pub doc: Option<String>,
    /// Selected entity addresses.
    pub selection: Vec<String>,
    /// Current command or turn.
    pub activity: Option<Task>,
}

/// A participant's current task.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Task {
    /// The command or turn.
    pub what: String,
    /// Why, in the participant's words.
    pub why: Option<String>,
}
