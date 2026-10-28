//! J6 favourites and scoped Assets searches through the real offscreen SDL route.
use super::input_verify::native;
use crate::{Editor, gpu::NativeUi, shell::Shell};
use ara_platform::{Event, Window};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

mod input;
mod proof;

const QUERY: &str = "type:scene";
const SAVED: &str = "Native scoped scenes مشاهد";
const PATIENCE: Duration = Duration::from_secs(60);
const FAVOURITE: &str = "assets-native-folder-favourite-select-recall";
const SEARCH: &str = "assets-native-scoped-search-prompt-select-recall";

struct Initial {
    folder: String,
    query: String,
    scope: bool,
    asset: Option<String>,
    maximized: Option<String>,
    focus: String,
}

pub(super) struct BookmarksCheck {
    output: PathBuf,
    phase: u32,
    started: Option<Instant>,
    finished: bool,
    restored: bool,
    initial: Option<Initial>,
    folder: String,
    authored: String,
    storage: String,
    revision: u64,
    documents: Value,
    nav_start: usize,
    navigation: Value,
    camera: Option<crate::state::Camera>,
    doc: String,
    selection: Vec<String>,
    native_events: usize,
    trace: Vec<Value>,
    results: Vec<Value>,
    pending_capture: Option<&'static str>,
    error: Option<String>,
    favourite_recalled: bool,
    search_recalled: bool,
    owns_keys: bool,
}

impl BookmarksCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Bookmarks verification requires the isolated behaviour fixture".into());
        }
        Ok(Self {
            output: output.into(),
            phase: 0,
            started: None,
            finished: false,
            restored: false,
            initial: None,
            folder: String::new(),
            authored: String::new(),
            storage: String::new(),
            revision: 0,
            documents: Value::Null,
            nav_start: 0,
            navigation: Value::Null,
            camera: None,
            doc: String::new(),
            selection: Vec::new(),
            native_events: 0,
            trace: Vec::new(),
            results: Vec::new(),
            pending_capture: None,
            error: None,
            favourite_recalled: false,
            search_recalled: false,
            owns_keys: false,
        })
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|row| row["ok"] != true)
            .filter_map(|row| row["journey"].as_str().map(str::to_owned))
            .collect()
    }

    pub fn validate(&self) -> ara_ui::Result<()> {
        if !self.finished
            || !self.restored
            || self.results.len() != 2
            || !self.failures().is_empty()
        {
            return Err(format!("Native Assets bookmarks incomplete or failed: phase {}, restored {}, failures {:?}", self.phase, self.restored, self.failures()).into());
        }
        Ok(())
    }

    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.finished || self.pending_capture.is_some() {
            return Ok(());
        }
        if self.error.is_some() || self.phase == 22 {
            let restored = self.restore(editor, shell);
            if let Err(error) = restored {
                self.error = Some(format!(
                    "{}; cleanup: {error}",
                    self.error.as_deref().unwrap_or("Bookmarks cleanup failed")
                ));
            }
            self.finish_results();
            self.finished = true;
            return self.write();
        }
        let before = events.len();
        let result = (|| {
            if !scale.is_finite() || scale <= 0. {
                return Err("Bookmarks native input scale is invalid".into());
            }
            if self
                .started
                .is_some_and(|started| started.elapsed() >= PATIENCE)
            {
                return Err(format!("Bookmarks proof timed out at phase {}; folder={}, query={:?}, scope={}, modal={}, popup={}", self.phase, editor.state.asset_folder, editor.state.searches.get("assets"), editor.state.asset_search_folder_only, shell.modal_active(), shell.choice_open()).into());
            }
            self.step(editor, shell, window, scale, events)
        })();
        self.native_events += events.len().saturating_sub(before);
        if let Err(error) = result {
            events.truncate(before);
            if self.initial.is_some() {
                native::tap(window, 41, events)?;
                native::focus_lost(window, events)?;
            }
            self.error = Some(error.to_string());
            self.mark("failure", editor, shell, json!({"error":self.error}));
        }
        Ok(())
    }

    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if let Some(stage) = self.pending_capture.take() {
            self.capture(stage, editor, shell, gpu)?;
            self.write()?;
        }
        Ok(())
    }

    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("bookmarks.json"),
            ara_json::to_canonical_string(&json!({
                "completed":self.finished,"restored":self.restored,"expected":2,"attempted":self.results.len(),
                "passed":self.results.iter().filter(|row|row["ok"]==true).count(),"failed":self.failures().len(),
                "phase":self.phase,"elapsedSeconds":self.started.map(|started|started.elapsed().as_secs_f64()),
                "nativeEvents":self.native_events,"error":self.error,"journeys":self.results,"trace":self.trace,
                "navigation":self.navigation,"beforeContentRevision":self.revision,"beforeDocuments":self.documents,
                "restoredFields":["Assets folder","Assets query","folder-only scope","inspected asset","maximized panel"],
                "setupLayout":"Assets is moved to the main view for native input; the fixture's docking layout is not asserted unchanged",
                "paths":{"storage":self.storage,"authored":self.authored,"folder":self.folder},
                "scope":"Raw SDL pointer/key/UTF-8 -> production router -> favourite toolbar/shared prompt/shared searchable Select. Setup and cleanup use typed navigation only; no authored fixture writes. Canonical content fingerprints are taken once before and after measured actions."
            })),
        )?;
        Ok(())
    }

    fn restore(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        let Some(initial) = &self.initial else {
            self.restored = true;
            return Ok(());
        };
        // Only the entries owned by this journey are removed; unrelated bookmarks survive.
        if self.owns_keys && editor.state.asset_saved_searches.contains_key(SAVED) {
            editor.action("editor.workspace", json!({"removeSearch":SAVED}))?;
        }
        if self.owns_keys && editor.state.asset_favourites.contains_key(&self.folder) {
            editor.action(
                "editor.workspace",
                json!({"favourite":self.folder,"favouriteFolder":true,"favouriteEnabled":false}),
            )?;
        }
        editor.action("editor.workspace", json!({"folder":initial.folder,"assetQuery":initial.query,"folderOnly":initial.scope,"asset":initial.asset.as_deref().unwrap_or("")}))?;
        editor.action(
            "editor.view",
            json!({"maximized":initial.maximized.as_deref().unwrap_or("")}),
        )?;
        editor.action("editor.focus", json!({"panel":initial.focus}))?;
        shell.rebuild(editor)?;
        self.restored = editor.state.asset_folder == initial.folder
            && editor
                .state
                .searches
                .get("assets")
                .map(String::as_str)
                .unwrap_or("")
                == initial.query
            && editor.state.asset_search_folder_only == initial.scope
            && editor.state.asset_path == initial.asset
            && editor.state.maximized == initial.maximized;
        Ok(())
    }
}
