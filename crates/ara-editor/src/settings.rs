//! Typed project settings and user preferences. Every edit is an undoable op.
//!
//! One definition table drives the Settings window, its search, `editor.settings`
//! over MCP and validation. Edits commit to a private op store (with per-author
//! undo), then persist: user scope to the preferences file chosen at open,
//! project policy to `settings/project.json`, with name/main metadata in `ara.project.json`.
use ara_doc::{AssetId, Document, Edit, RecordId, Workspace};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

mod project_live;

/// Where a setting is stored and who shares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// Preferences for this person, on every project.
    User,
    /// Shared project policy (`settings/project.json`); name/main remain in the marker.
    Project,
}
/// The control a setting uses and the values it accepts.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// On/off checkbox.
    Toggle,
    /// One of `(value, label)`.
    Choice(&'static [(&'static str, &'static str)]),
    /// A bounded number.
    Number {
        /// Minimum.
        min: f64,
        /// Maximum.
        max: f64,
        /// Increment.
        step: f64,
        /// Unit shown inside the field.
        unit: &'static str,
    },
    /// Free text.
    Text,
    /// A string map edited on its own page (keyboard shortcuts).
    Map,
    /// Up to `max` linear RGBA colours (`[[r, g, b, a], ...]`), edited from the colour
    /// picker; Settings shows them and resets them.
    Colors {
        /// Longest list kept.
        max: usize,
    },
}
/// A default value, written as a literal in the table.
#[derive(Clone, Copy, Debug)]
pub enum Lit {
    /// Boolean.
    B(bool),
    /// String or choice value.
    S(&'static str),
    /// Number.
    N(f64),
    /// Empty object.
    Empty,
    /// Empty list.
    None,
}
impl Lit {
    fn value(self) -> Value {
        match self {
            Self::B(v) => json!(v),
            Self::S(v) => json!(v),
            Self::N(v) => json!(v),
            Self::Empty => json!({}),
            Self::None => json!([]),
        }
    }
}
/// One setting row.
#[derive(Clone, Copy, Debug)]
pub struct Def {
    /// Stable key, e.g. `appearance.theme`.
    pub key: &'static str,
    /// Storage scope.
    pub scope: Scope,
    /// Category in the Settings sidebar.
    pub category: &'static str,
    /// Sentence-case label.
    pub label: &'static str,
    /// One-line explanation shown under the row and as its tooltip.
    pub description: &'static str,
    /// Control and validation.
    pub kind: Kind,
    /// Value when unset.
    pub default: Lit,
}
const fn def(
    key: &'static str,
    scope: Scope,
    category: &'static str,
    label: &'static str,
    description: &'static str,
    kind: Kind,
    default: Lit,
) -> Def {
    Def {
        key,
        scope,
        category,
        label,
        description,
        kind,
        default,
    }
}
use Scope::{Project, User};
/// `viewport.quality` choices: the project's default, Auto, or a preset.
pub const QUALITY_OVERRIDES: &[(&str, &str)] = &[
    ("project", "Project default"),
    ("auto", "Auto"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("epic", "Epic"),
];
/// The most project colour swatches `color.swatches` keeps.
pub const COLOR_SWATCHES: usize = 48;
/// The most recent colours `color.recent` keeps.
pub const RECENT_COLORS: usize = 12;
/// Every setting, grouped by category in display order.
pub const DEFS: &[Def] = &[
    def(
        "scripts.ide",
        User,
        "Scripts",
        "Script editor",
        "Executable path for Open in IDE. Empty tries VS Code, then the system handler; system always uses the file association.",
        Kind::Text,
        Lit::S(""),
    ),
    def(
        "appearance.theme",
        User,
        "Appearance",
        "Theme",
        "Ara Dark or Ara Light semantic tokens. The native window frame follows.",
        Kind::Choice(&[("dark", "Dark"), ("light", "Light")]),
        Lit::S("dark"),
    ),
    def(
        "appearance.density",
        User,
        "Appearance",
        "Density",
        "Compact uses 22 px rows; Comfortable adds 4 px to rows and controls.",
        Kind::Choice(&[("compact", "Compact"), ("comfortable", "Comfortable")]),
        Lit::S("compact"),
    ),
    def(
        "appearance.uiScale",
        User,
        "Appearance",
        "UI scale",
        "Scales every panel on top of the monitor's display scale.",
        Kind::Number {
            min: 0.75,
            max: 2.,
            step: 0.05,
            unit: "×",
        },
        Lit::N(1.),
    ),
    def(
        "appearance.reduceMotion",
        User,
        "Appearance",
        "Reduce motion",
        "Turns off decorative transitions; all interaction feedback stays.",
        Kind::Toggle,
        Lit::B(false),
    ),
    def(
        "viewport.quality",
        User,
        "Viewport",
        "Scene quality",
        "Lit quality of your Scene view: the project's default, detected from the GPU, or a preset. Low is the raster preview; never saved in the project.",
        Kind::Choice(QUALITY_OVERRIDES),
        Lit::S("project"),
    ),
    def(
        "viewport.grid",
        User,
        "Viewport",
        "Show grid",
        "Adaptive ground grid with origin axes.",
        Kind::Toggle,
        Lit::B(true),
    ),
    def(
        "viewport.outline",
        User,
        "Viewport",
        "Selection outline",
        "Screen-space outline around selected entities.",
        Kind::Toggle,
        Lit::B(true),
    ),
    def(
        "viewport.gizmoSize",
        User,
        "Viewport",
        "Gizmo size",
        "Transform handle size relative to the 88 px default.",
        Kind::Number {
            min: 60.,
            max: 140.,
            step: 5.,
            unit: "%",
        },
        Lit::N(100.),
    ),
    def(
        "viewport.cameraSpeed",
        User,
        "Viewport",
        "Camera speed",
        "Fly speed with the right mouse button held; Shift triples it.",
        Kind::Number {
            min: 0.5,
            max: 50.,
            step: 0.5,
            unit: "m/s",
        },
        Lit::N(4.),
    ),
    def(
        "viewport.surfaceSnap",
        User,
        "Viewport",
        "Surface snapping",
        "Move selected geometry onto the actual mesh under the pointer.",
        Kind::Toggle,
        Lit::B(false),
    ),
    def(
        "viewport.vertexSnap",
        User,
        "Viewport",
        "Vertex snapping",
        "Snap a selected mesh vertex to a target vertex. Hold V temporarily while editing the Scene.",
        Kind::Toggle,
        Lit::B(false),
    ),
    def(
        "viewport.surfaceOffset",
        User,
        "Viewport",
        "Surface offset",
        "Distance from the target surface along its geometric normal.",
        Kind::Number {
            min: -1000.,
            max: 1000.,
            step: 0.01,
            unit: "m",
        },
        Lit::N(0.),
    ),
    def(
        "input.keymap",
        User,
        "Keyboard shortcuts",
        "Keymap preset",
        "Bindings for shared concepts from another editor. Your own bindings stay on top.",
        Kind::Choice(&[
            ("ara", "Ara"),
            ("unreal", "Unreal"),
            ("unity", "Unity"),
            ("blender", "Blender"),
        ]),
        Lit::S("ara"),
    ),
    def(
        "input.shortcuts",
        User,
        "Keyboard shortcuts",
        "Custom shortcuts",
        "Per-command bindings that replace the preset's.",
        Kind::Map,
        Lit::Empty,
    ),
    def(
        "hierarchy.filters",
        User,
        "Hierarchy",
        "Saved hierarchy filters",
        "Named name, ID and component queries, managed from the Hierarchy toolbar.",
        Kind::Map,
        Lit::Empty,
    ),
    def(
        "color.swatches",
        Project,
        "Colours",
        "Colour swatches",
        "The project's saved colours, shared by everyone. Add them from the colour picker.",
        Kind::Colors {
            max: COLOR_SWATCHES,
        },
        Lit::None,
    ),
    def(
        "color.recent",
        User,
        "Colours",
        "Recent colours",
        "Colours you picked lately, newest first, across projects.",
        Kind::Colors { max: RECENT_COLORS },
        Lit::None,
    ),
    def(
        "performance.frameCap",
        User,
        "Performance",
        "Editor frame cap",
        "Upper bound for editor redraws. Idle frames do no work either way.",
        Kind::Choice(&[("144", "144 Hz"), ("120", "120 Hz"), ("60", "60 Hz")]),
        Lit::S("144"),
    ),
    def(
        "project.name",
        Project,
        "General",
        "Project name",
        "Shown in the title bar and the breadcrumb.",
        Kind::Text,
        Lit::S(""),
    ),
    def(
        "project.mainScene",
        Project,
        "General",
        "Main scene",
        "Scene opened at startup and played by default (project path).",
        Kind::Text,
        Lit::S(""),
    ),
    def(
        "build.profile",
        Project,
        "Build & export",
        "Build profile",
        "Selected export profile. Empty uses the default profile, or the first indexed profile when no default exists.",
        Kind::Text,
        Lit::S(""),
    ),
    def(
        "rendering.quality",
        Project,
        "Rendering",
        "Quality",
        "Default quality for Play, exported players and the Scene view. Auto picks High on ray-tracing GPUs (Medium under 8 GiB), Low otherwise; Epic is never automatic.",
        Kind::Choice(&[
            ("auto", "Auto"),
            ("low", "Low"),
            ("medium", "Medium"),
            ("high", "High"),
            ("epic", "Epic"),
        ]),
        Lit::S("auto"),
    ),
    def(
        "play.mode",
        Project,
        "Networking & Play",
        "Play mode",
        "Local, a listen host with clients, or a dedicated server with clients (D19).",
        Kind::Choice(&[
            ("local", "Local"),
            ("host", "Host + clients"),
            ("dedicated", "Dedicated + clients"),
        ]),
        Lit::S("local"),
    ),
    def(
        "play.clients",
        Project,
        "Networking & Play",
        "Clients",
        "Additional client processes for Host and Dedicated modes.",
        Kind::Number {
            min: 0.,
            max: 16.,
            step: 1.,
            unit: "",
        },
        Lit::N(1.),
    ),
    def(
        "play.tickHz",
        Project,
        "Time & simulation",
        "Fixed tick rate",
        "Simulation steps per second. Changing it changes determinism hashes.",
        Kind::Choice(&[("30", "30 Hz"), ("60", "60 Hz"), ("120", "120 Hz")]),
        Lit::S("60"),
    ),
    def(
        "play.processMode",
        Project,
        "Networking & Play",
        "Play processes",
        "Run independent player worlds in one process or a separate process per player (D19).",
        Kind::Choice(&[
            ("separate", "Separate process per player"),
            ("shared", "Shared process, independent worlds"),
        ]),
        Lit::S("separate"),
    ),
    def(
        "play.networkProfiles",
        Project,
        "Networking & Play",
        "Client network profiles",
        "Choose per-client network conditions in Game Play options.",
        Kind::Map,
        Lit::Empty,
    ),
    def(
        "play.startPaused",
        Project,
        "Time & simulation",
        "Start paused",
        "Play stops at tick 0 so you can step explicitly.",
        Kind::Toggle,
        Lit::B(false),
    ),
    def(
        "play.seed",
        Project,
        "Time & simulation",
        "Session seed",
        "Seed for every deterministic random stream in Play.",
        Kind::Number {
            min: 0.,
            max: 4_294_967_295.,
            step: 1.,
            unit: "",
        },
        Lit::N(0.),
    ),
    def(
        "play.inGameView",
        User,
        "Viewport",
        "Play in the Game view",
        "Play shows inside the editor's Game view (input goes to it while focused). Off opens a separate player window.",
        Kind::Toggle,
        Lit::B(true),
    ),
    def(
        "play.hotReload",
        Project,
        "Networking & Play",
        "Hot-reload assets in Play",
        "Pushes newly imported asset generations into the running world.",
        Kind::Toggle,
        Lit::B(true),
    ),
];
/// Look up a definition.
pub fn def_of(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}
/// Category names in display order for a scope.
pub fn categories(scope: Scope) -> Vec<&'static str> {
    let mut out: Vec<&str> = Vec::new();
    for d in DEFS.iter().filter(|d| d.scope == scope) {
        if !out.contains(&d.category) {
            out.push(d.category);
        }
    }
    out
}
/// Opaque settings payload stored in the private op document.
#[derive(Schema)]
#[ara(component)]
#[allow(dead_code)] // Schema declaration; values are a canonical JSON object.
struct EditorSettings {
    /// Non-default values keyed by setting key.
    values: Value,
}
/// Settings values with op history; owned by the editor service.
pub struct Settings {
    store: Store,
    doc: AssetId,
    project_path: PathBuf,
    preferences_path: PathBuf,
    /// Non-default user values.
    pub user: Map<String, Value>,
    /// Non-default project values.
    pub project: Map<String, Value>,
    /// Last validated Host policy, separate from local settings history.
    observed_project: Map<String, Value>,
    /// Content revision consumed by the live projection; idle ticks do no work.
    observed_host_revision: Option<u64>,
    failures: Vec<String>,
    /// Writes the preferences file off the UI thread; `None` keeps them in memory.
    user_writer: Option<crate::persist::BackgroundWriter<Value>>,
}
const USER: &str = "e_userpref";
const PROJECT: &str = "e_projpref";
impl Settings {
    /// Load both scopes. `user_path: None` keeps preferences in memory (tests).
    pub fn open(root: &Path, user_path: Option<PathBuf>) -> anyhow::Result<Self> {
        let mut user = user_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        let project_path = root.join("ara.project.json");
        let preferences_path = root.join(ara_host::project_policy::PREFERENCES);
        let manifest = std::fs::read_to_string(&project_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or(Value::Null);
        let mut project = ara_host::project_policy::read(root, "settings")
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
            .and_then(|settings| settings.as_object().cloned())
            .unwrap_or_default();
        for (key, field) in [("project.name", "name"), ("project.mainScene", "main")] {
            project.remove(key);
            if let Some(v) = manifest[field].as_str() {
                project.insert(key.into(), v.into());
            }
        }
        // The runtime uses this same per-section canonical/legacy policy reader.
        if let Some(rendering) = ara_host::project_policy::read(root, "rendering")
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
        {
            project.remove("rendering.quality");
            if let Some(quality) = rendering.get("quality").and_then(Value::as_str) {
                project.insert("rendering.quality".into(), quality.into());
            }
        }
        // Before presets: Fidelity meant the project's default, Raster meant Low.
        match user.get("viewport.quality").and_then(Value::as_str) {
            Some("fidelity") => {
                user.remove("viewport.quality");
            }
            Some("raster") => {
                user.insert("viewport.quality".into(), "low".into());
            }
            _ => {}
        }
        // Unknown or invalid stored values fall back to their defaults.
        user.retain(|k, v| def_of(k).is_some_and(|d| d.scope == User && valid(d, v).is_ok()));
        project.retain(|k, v| def_of(k).is_some_and(|d| d.scope == Project && valid(d, v).is_ok()));
        let mut db = SchemaDb::new();
        ara_doc::register(&mut db);
        db.register::<EditorSettings>();
        let doc = AssetId::parse("sc_settings").map_err(anyhow::Error::msg)?;
        let mut document = Document::new("scene", doc.clone());
        for (id, values) in [(USER, &user), (PROJECT, &project)] {
            document.insert_unchecked(
                RecordId::parse(id).map_err(anyhow::Error::msg)?,
                json!({"EditorSettings":{"values":values}})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        let mut ws = Workspace::default();
        ws.insert(".ara/editor-settings.scene", document);
        let user_writer = user_path.map(|path| {
            crate::persist::BackgroundWriter::new("preferences-writer", move |user: Value| {
                crate::persist::write_text(&path, &ara_json::to_canonical_string(&user))
                    .map_err(|e| format!("Preferences were not saved to {}: {e}", path.display()))
            })
        });
        Ok(Self {
            store: Store::new(db, ws),
            doc,
            project_path,
            preferences_path,
            user,
            observed_project: project.clone(),
            observed_host_revision: None,
            project,
            failures: Vec::new(),
            user_writer,
        })
    }
    /// Effective value: stored or default.
    pub fn value(&self, key: &str) -> Value {
        let Some(d) = def_of(key) else {
            return Value::Null;
        };
        self.scope_values(d.scope)
            .get(key)
            .cloned()
            .unwrap_or_else(|| d.default.value())
    }
    /// Every effective value, for the navigation snapshot.
    pub fn snapshot(&self) -> std::collections::BTreeMap<String, Value> {
        DEFS.iter()
            .map(|d| (d.key.to_owned(), self.value(d.key)))
            .collect()
    }
    /// Whether a setting differs from its default.
    pub fn modified(&self, key: &str) -> bool {
        def_of(key).is_some_and(|d| self.scope_values(d.scope).contains_key(key))
    }
    fn scope_values(&self, scope: Scope) -> &Map<String, Value> {
        match scope {
            User => &self.user,
            Project => &self.project,
        }
    }
    /// Set (`Some`) or reset (`None`) one setting as an attributed, undoable op.
    pub fn set(
        &mut self,
        key: &str,
        value: Option<Value>,
        author: Author,
    ) -> Result<String, Vec<ara_diag::Diagnostic>> {
        let d = def_of(key).ok_or_else(|| vec![crate::invalid(format!("no setting `{key}`"))])?;
        let (value, label) = match value {
            Some(v) => {
                valid(d, &v).map_err(|e| vec![crate::invalid(e)])?;
                let value = (v != d.default.value()).then_some(v);
                (value, format!("Set {}", d.label))
            }
            None => (None, format!("Reset {}", d.label)),
        };
        if value.as_ref() == self.scope_values(d.scope).get(key) {
            return Ok(label);
        }
        let mut meta = TxnMeta::by(author);
        meta.label = Some(label.clone());
        meta.why = Some(label.clone());
        let edit = Edit::Set {
            doc: self.doc.clone(),
            id: RecordId::parse(if d.scope == User { USER } else { PROJECT })
                .map_err(|e| vec![crate::invalid(e)])?,
            path: FieldPath::field("EditorSettings").child("values").key(key),
            value,
        };
        self.store.transact(meta, [edit])?;
        self.reload(Some(d.scope));
        Ok(label)
    }
    /// Undo or redo `author`'s latest settings edit; returns its label.
    pub fn undo(
        &mut self,
        author: &Author,
        redo: bool,
    ) -> Result<String, Vec<ara_diag::Diagnostic>> {
        let label = self.next_label(author, redo).unwrap_or_default();
        self.check_history_values(author, redo)?;
        if redo {
            self.store.redo_last(author)?;
        } else {
            self.store.undo_last(author)?;
        }
        self.reload(None);
        Ok(label)
    }
    /// Original action title for Undo/Redo, following typed inverse transactions.
    pub fn next_label(&self, author: &Author, redo: bool) -> Option<String> {
        let txn = if redo {
            self.store.next_redo(author)?
        } else {
            self.store.next_undo(author)?
        };
        crate::transaction_label::original(&self.store, txn)?
            .label
            .clone()
    }
    fn reload(&mut self, changed: Option<Scope>) {
        let read = |id: &str| {
            self.store
                .workspace()
                .doc(self.doc.as_str())
                .and_then(|d| d.record(id))
                .and_then(|r| r.get("EditorSettings"))
                .and_then(|v| v.get("values"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        };
        let user = read(USER);
        let project = read(PROJECT);
        let user_changed = user != self.user;
        let project_changed = project != self.project;
        self.user = user;
        let previous_project = std::mem::replace(&mut self.project, project);
        if user_changed || changed == Some(User) {
            self.persist_user();
        }
        if project_changed || changed == Some(Project) {
            self.persist_project(&previous_project);
        }
    }
    /// Write failures since the last call (the editor service logs them).
    pub fn take_failures(&mut self) -> Vec<String> {
        let mut failures = std::mem::take(&mut self.failures);
        if let Some(writer) = &self.user_writer {
            failures.extend(writer.take_failures());
        }
        failures
    }
    /// Hand the preferences to their writer thread (`crate::persist`): the file is
    /// written off the UI thread, newest value first, flushed when the editor closes.
    fn persist_user(&mut self) {
        if let Some(writer) = &self.user_writer {
            writer.save(Value::Object(self.user.clone()));
        }
    }
    fn persist_project(&mut self, previous: &Map<String, Value>) {
        if let Err(error) = self.save_project(previous) {
            self.failures.push(error.to_string());
        }
    }
    fn save_project(&self, previous: &Map<String, Value>) -> anyhow::Result<()> {
        let root = self
            .project_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Project marker has no parent"))?;
        // The public reader exposes one effective top-level policy key. Reading
        // the canonical object separately is only for lossless save preservation.
        let mut preferences = read_project_object(&self.preferences_path)?;
        const POLICY: &[&str] = &["settings", "rendering", "input", "physics", "look", "ui"];
        for key in POLICY {
            if !preferences.contains_key(*key)
                && let Some(value) = ara_host::project_policy::read(root, key)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?
            {
                preferences.insert((*key).into(), value);
            }
        }
        let changed = |key: &str| self.project.get(key) != previous.get(key);
        let mut settings = preferences
            .get("settings")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut settings_changed = false;
        for definition in DEFS.iter().filter(|definition| {
            definition.scope == Project
                && !matches!(
                    definition.key,
                    "project.name" | "project.mainScene" | "rendering.quality"
                )
        }) {
            if !changed(definition.key) {
                continue;
            }
            settings_changed = true;
            match self.project.get(definition.key) {
                Some(value) => {
                    settings.insert(definition.key.into(), value.clone());
                }
                None => {
                    settings.remove(definition.key);
                }
            }
        }
        if settings_changed {
            // An explicit empty section suppresses legacy fallback even if the
            // marker cleanup fails after the policy file has already landed.
            preferences.insert("settings".into(), Value::Object(settings));
        }
        if changed("rendering.quality") {
            let mut rendering = preferences
                .get("rendering")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            match self.project.get("rendering.quality") {
                Some(value) => {
                    rendering.insert("quality".into(), value.clone());
                }
                None => {
                    rendering.remove("quality");
                }
            }
            preferences.insert("rendering".into(), Value::Object(rendering));
        }
        let preserved: Vec<_> = POLICY
            .iter()
            .copied()
            .filter(|key| preferences.contains_key(*key))
            .collect();
        write_atomic(&self.preferences_path, &Value::Object(preferences)).map_err(|error| {
            anyhow::anyhow!(
                "Project policy was not saved to {}: {error}",
                self.preferences_path.display()
            )
        })?;
        // Re-read metadata before its own atomic save rather than replaying an
        // opening snapshot over unrelated agent or tool changes.
        let mut manifest = read_project_object(&self.project_path)?;
        for (key, field) in [("project.name", "name"), ("project.mainScene", "main")] {
            if !changed(key) {
                continue;
            }
            match self.project.get(key) {
                Some(value) => {
                    manifest.insert(field.into(), value.clone());
                }
                None => {
                    manifest.remove(field);
                }
            }
        }
        for key in preserved {
            manifest.remove(key);
        }
        write_atomic(&self.project_path, &Value::Object(manifest)).map_err(|error| {
            anyhow::anyhow!(
                "Project metadata was not saved to {}: {error}",
                self.project_path.display()
            )
        })
    }
}
/// Preserve unknown canonical policy members without creating optional files on read.
fn read_project_object(path: &Path) -> anyhow::Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(anyhow::anyhow!("{}: {error}", path.display())),
    };
    match ara_json::parse(&text).map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))? {
        Value::Object(object) => Ok(object),
        _ => Err(anyhow::anyhow!(
            "{}: project settings must be a JSON object",
            path.display()
        )),
    }
}
pub(crate) fn write_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(
        &temp,
        ara_json::to_canonical_string(&ara_json::sort_keys(value)),
    )?;
    std::fs::rename(temp, path)
}
/// Validate a value against its definition.
pub fn valid(d: &Def, value: &Value) -> Result<(), String> {
    match d.kind {
        Kind::Toggle if value.is_boolean() => Ok(()),
        Kind::Choice(options)
            if value
                .as_str()
                .is_some_and(|v| options.iter().any(|(o, _)| *o == v)) =>
        {
            Ok(())
        }
        Kind::Number { min, max, .. } => match value.as_f64() {
            Some(v) if v.is_finite() && (min..=max).contains(&v) => Ok(()),
            _ => Err(format!("{} must be a number from {min} to {max}", d.label)),
        },
        Kind::Text if value.is_string() => Ok(()),
        Kind::Map
            if value
                .as_object()
                .is_some_and(|m| m.values().all(Value::is_string)) =>
        {
            Ok(())
        }
        Kind::Colors { max } => match value.as_array() {
            Some(list)
                if list.len() <= max
                    && list.iter().all(|c| {
                        c.as_array().is_some_and(|c| {
                            matches!(c.len(), 3 | 4)
                                && c.iter()
                                    .all(|v| v.as_f64().is_some_and(|v| v.is_finite() && v >= 0.))
                        })
                    }) =>
            {
                Ok(())
            }
            _ => Err(format!(
                "{} must be at most {max} linear colours, each [r, g, b] or [r, g, b, a] from 0",
                d.label
            )),
        },
        Kind::Choice(options) => Err(format!(
            "{} must be one of {}",
            d.label,
            options
                .iter()
                .map(|(o, _)| *o)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(format!("{} has the wrong type", d.label)),
    }
}
/// The user preferences file, `editor-settings.json` in [`config_dir`].
pub fn user_settings_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("editor-settings.json"))
}

/// The user config directory: `ARA_CONFIG_DIR` when set (tests, captures and
/// verification routes, portable installs), else the OS profile (`%APPDATA%\Ara`,
/// `~/Library/Application Support/Ara`, `~/.config/Ara`). Tests and cargo-launched
/// processes are refused the OS profile and fail instead: they never touch the
/// owner's preferences (`.cargo/config.toml` points them under `target/`).
pub fn config_dir() -> anyhow::Result<PathBuf> {
    resolve_config_dir(
        std::env::var_os("ARA_CONFIG_DIR"),
        cfg!(test) || std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
        os_config_dir(),
    )
}

/// `ARA_CONFIG_DIR`, else the OS profile unless `guarded` (a test or cargo run).
fn resolve_config_dir(
    explicit: Option<std::ffi::OsString>,
    guarded: bool,
    os: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    if let Some(dir) = explicit.filter(|d| !d.to_string_lossy().trim().is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    anyhow::ensure!(
        !guarded,
        "tests and cargo-launched runs must set ARA_CONFIG_DIR to an isolated directory \
         under target/; they never use the owner's profile ({})",
        os.as_deref()
            .map_or_else(|| "unknown".into(), |p| p.display().to_string())
    );
    os.ok_or_else(|| anyhow::anyhow!("cannot find the user config directory; set ARA_CONFIG_DIR"))
}

fn os_config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("Ara"))
}

#[cfg(test)]
#[path = "settings/live_tests.rs"]
mod live_tests;

#[cfg(test)]
#[path = "settings/history_live_tests.rs"]
mod history_live_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Project(PathBuf);
    impl Project {
        fn new(marker: Value, policy: Option<Value>) -> Self {
            let parent = std::path::absolute(crate::test_target())
                .expect("absolute test target")
                .join("editor-project-policy");
            std::fs::create_dir_all(&parent).expect("test parent");
            let root = parent.join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).expect("fresh project");
            let project = Self(root);
            project.write("ara.project.json", &marker);
            if let Some(policy) = policy {
                project.write(ara_host::project_policy::PREFERENCES, &policy);
            }
            project
        }
        fn write(&self, path: &str, value: &Value) {
            write_atomic(&self.0.join(path), value).expect("fixture JSON");
        }
        fn read(&self, path: &str) -> Value {
            ara_json::parse(&std::fs::read_to_string(self.0.join(path)).expect("read fixture"))
                .expect("fixture JSON")
        }
        fn open(&self) -> Settings {
            Settings::open(&self.0, None).expect("settings")
        }
    }
    impl Drop for Project {
        fn drop(&mut self) {
            assert!(
                self.0.starts_with(
                    std::path::absolute(crate::test_target())
                        .expect("test target")
                        .join("editor-project-policy")
                )
            );
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn tests_never_resolve_the_os_profile() {
        let profile = PathBuf::from("C:/Users/owner/AppData/Roaming/Ara");
        let isolated = std::ffi::OsString::from("target/ara-config");
        assert!(resolve_config_dir(None, true, Some(profile.clone())).is_err());
        assert!(resolve_config_dir(Some(" ".into()), true, Some(profile.clone())).is_err());
        assert_eq!(
            resolve_config_dir(Some(isolated), true, Some(profile.clone())).expect("isolated"),
            PathBuf::from("target/ara-config")
        );
        assert_eq!(
            resolve_config_dir(None, false, Some(profile.clone())).expect("installed editor"),
            profile
        );
        // This process is a test: whatever it resolves is not the OS profile.
        if let (Ok(dir), Some(os)) = (config_dir(), os_config_dir()) {
            assert_ne!(dir, os);
        }
    }
    #[test]
    fn edits_validate_persist_and_undo_per_author() {
        let project = Project::new(json!({"main":"levels/main.scene","name":"Outpost"}), None);
        let root = &project.0;
        let user = root.join("user.json");
        let mut s = Settings::open(root, Some(user.clone())).expect("open");
        assert_eq!(s.value("project.name"), json!("Outpost"));
        assert!(
            s.set("appearance.theme", Some(json!("blue")), Author::Human)
                .is_err()
        );
        s.set("appearance.theme", Some(json!("light")), Author::Human)
            .expect("set");
        s.set("play.tickHz", Some(json!("120")), Author::Human)
            .expect("set");
        assert!(s.modified("appearance.theme"));
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("ara.project.json")).expect("read"),
        )
        .expect("json");
        let preferences = project.read(ara_host::project_policy::PREFERENCES);
        assert_eq!(preferences["settings"]["play.tickHz"], "120");
        assert!(manifest.get("settings").is_none());
        assert_eq!(manifest["main"], "levels/main.scene");
        assert_eq!(
            s.next_label(&Author::Human, false).as_deref(),
            Some("Set Fixed tick rate")
        );
        s.undo(&Author::Human, false).expect("undo");
        assert_eq!(s.value("play.tickHz"), json!("60"));
        s.undo(&Author::Human, true).expect("redo");
        assert_eq!(s.value("play.tickHz"), json!("120"));
        // Closing flushes background preference saves before a new editor reads them.
        drop(s);
        let reopened = Settings::open(root, Some(user)).expect("reopen");
        assert_eq!(reopened.value("appearance.theme"), json!("light"));
        drop(reopened);
    }

    #[test]
    fn optional_empty_policy_and_omitted_sections_use_the_public_legacy_fallback_without_writes() {
        let marker = json!({"name":"Outpost","main":"levels/main.scene","layoutVersion":3,
            "settings":{"play.tickHz":"120","build.profile":"legacy-profile"},"rendering":{"quality":"high"},"ui":{"main":"ui/legacy.ui"}});
        for policy in [
            None,
            Some(json!({})),
            Some(json!({"ui":{"main":"ui/canonical.ui"}})),
        ] {
            let project = Project::new(marker.clone(), policy.clone());
            let settings = project.open();
            assert_eq!(settings.value("project.name"), json!("Outpost"));
            assert_eq!(
                settings.value("project.mainScene"),
                json!("levels/main.scene")
            );
            assert_eq!(settings.value("play.tickHz"), json!("120"));
            assert_eq!(settings.value("build.profile"), json!("legacy-profile"));
            assert_eq!(settings.value("rendering.quality"), json!("high"));
            assert_eq!(project.read("ara.project.json"), marker);
            if let Some(policy) = policy {
                assert_eq!(project.read(ara_host::project_policy::PREFERENCES), policy);
            } else {
                assert!(
                    !project
                        .0
                        .join(ara_host::project_policy::PREFERENCES)
                        .exists()
                );
            }
            assert!(!project.0.join(ara_doc::AUTHORING_ROOT).exists());
            assert!(!project.0.join(".ara").exists());
        }
        // The reader's precedence is per top-level key; an explicit section or
        // null does not inherit individual members of the legacy section.
        let project = Project::new(marker, Some(json!({"settings":{},"rendering":null})));
        let settings = project.open();
        assert_eq!(settings.value("play.tickHz"), json!("60"));
        assert_eq!(settings.value("build.profile"), json!(""));
        assert_eq!(settings.value("rendering.quality"), json!("auto"));
    }

    #[test]
    fn project_save_and_undo_preserve_unknown_policy_and_unmodified_latest_disk_settings() {
        let marker = json!({"id":"project-identity","name":"Outpost","main":"levels/main.scene","layoutVersion":3,"engine":"preview4",
            "settings":{"play.tickHz":"30"},"rendering":{"quality":"epic"},"ui":{"main":"legacy.ui"},"input":{"legacy":true},"physics":{"gravity":[0,-1,0]},"look":{"legacy":true}});
        let initial = json!({"settings":{"play.tickHz":"120","build.profile":"original-profile","plugin.option":{"enabled":true}},
            "rendering":{"quality":"high","extension":{"toneMapper":"custom"}},"ui":{"main":"ui/main.ui","unknown":[1,2,3]},
            "input":null,"physics":{"gravity":[0,-9.81,0],"custom":true},"look":{"camera":"custom"},"plugin":{"keep":"unchanged"}});
        let project = Project::new(marker, Some(initial.clone()));
        let mut settings = project.open();
        let mut latest = initial;
        latest["settings"]["build.profile"] = json!("agent-selected-profile");
        latest["settings"]["agent.option"] = json!({"preserve":true});
        latest["ui"]["newFromAgent"] = json!("keep");
        project.write(ara_host::project_policy::PREFERENCES, &latest);
        settings
            .set("play.tickHz", Some(json!("30")), Author::Human)
            .expect("typed setting");
        let mut expected = latest.clone();
        expected["settings"]["play.tickHz"] = json!("30");
        assert_eq!(
            project.read(ara_host::project_policy::PREFERENCES),
            expected
        );
        assert_eq!(
            project.read("ara.project.json"),
            json!({"id":"project-identity","name":"Outpost","main":"levels/main.scene","layoutVersion":3,"engine":"preview4"})
        );
        settings.undo(&Author::Human, false).expect("undo");
        assert_eq!(project.read(ara_host::project_policy::PREFERENCES), latest);
        settings.undo(&Author::Human, true).expect("redo");
        assert_eq!(
            project.read(ara_host::project_policy::PREFERENCES),
            expected
        );
        settings
            .set("rendering.quality", Some(json!("medium")), Author::Human)
            .expect("quality");
        expected["rendering"]["quality"] = json!("medium");
        assert_eq!(
            project.read(ara_host::project_policy::PREFERENCES),
            expected
        );
        settings.undo(&Author::Human, false).expect("quality undo");
        expected["rendering"]["quality"] = json!("high");
        assert_eq!(
            project.read(ara_host::project_policy::PREFERENCES),
            expected
        );
        assert!(settings.take_failures().is_empty());
        let reopened = project.open();
        assert_eq!(
            reopened.value("build.profile"),
            json!("agent-selected-profile")
        );
    }

    #[test]
    fn default_resets_keep_explicit_barriers_and_migrate_legacy_policy_intact() {
        let marker = json!({"name":"Outpost","main":"levels/main.scene","layoutVersion":3,"settings":{"play.tickHz":"120"},
            "rendering":{"quality":"high"},"input":{"actions":{"Jump":"Space"}},"physics":{"gravity":[0,-9.81,0]},"look":{"sensitivity":0.8},"ui":{"main":"ui/main.ui"}});
        let project = Project::new(marker.clone(), Some(json!({})));
        let mut settings = project.open();
        settings
            .set("play.tickHz", None, Author::Human)
            .expect("tick reset");
        settings
            .set("rendering.quality", None, Author::Human)
            .expect("quality reset");
        let policy = project.read(ara_host::project_policy::PREFERENCES);
        assert_eq!(policy["settings"], json!({}));
        assert_eq!(policy["rendering"], json!({}));
        for section in ["input", "physics", "look", "ui"] {
            assert_eq!(policy[section], marker[section]);
        }
        assert_eq!(
            project.read("ara.project.json"),
            json!({"name":"Outpost","main":"levels/main.scene","layoutVersion":3})
        );
        // A leftover/reintroduced legacy marker is the state after a failed
        // cleanup; an empty canonical section must not resurrect its old value.
        project.write("ara.project.json", &marker);
        let reopened = project.open();
        assert_eq!(reopened.value("play.tickHz"), json!("60"));
        assert_eq!(reopened.value("rendering.quality"), json!("auto"));
        assert_eq!(
            ara_host::project_policy::read(&project.0, "rendering").expect("policy"),
            Some(json!({}))
        );
        assert!(settings.take_failures().is_empty());
        assert!(!project.0.join(ara_doc::AUTHORING_ROOT).exists());
    }

    #[test]
    fn marker_identity_edits_do_not_accept_canonical_identity_aliases_or_erase_metadata() {
        let marker = json!({"id":"identity","name":"Outpost","main":"levels/main.scene","layoutVersion":3,"extension":{"owner":"plugin"}});
        let policy = json!({"settings":{"project.name":"Ignored","project.mainScene":"ignored.scene"},"ui":null,"plugin":{"keep":1}});
        let project = Project::new(marker.clone(), Some(policy.clone()));
        let mut settings = project.open();
        assert_eq!(settings.value("project.name"), json!("Outpost"));
        assert_eq!(
            settings.value("project.mainScene"),
            json!("levels/main.scene")
        );
        settings
            .set("project.name", Some(json!("Renamed")), Author::Human)
            .expect("rename");
        let mut expected = marker.clone();
        expected["name"] = json!("Renamed");
        assert_eq!(project.read("ara.project.json"), expected);
        assert_eq!(project.read(ara_host::project_policy::PREFERENCES), policy);
        settings.undo(&Author::Human, false).expect("undo");
        assert_eq!(project.read("ara.project.json"), marker);
        assert_eq!(project.read(ara_host::project_policy::PREFERENCES), policy);
    }

    #[test]
    fn malformed_policy_after_open_is_reported_without_overwriting_it_or_cleaning_the_marker() {
        let marker = json!({"name":"Outpost","main":"levels/main.scene","settings":{"play.tickHz":"120"},"physics":{"custom":true}});
        let project = Project::new(marker.clone(), Some(json!({})));
        let mut settings = project.open();
        let path = project.0.join(ara_host::project_policy::PREFERENCES);
        std::fs::write(&path, "{broken").expect("malformed external edit");
        settings
            .set("play.tickHz", Some(json!("30")), Author::Human)
            .expect("private typed transaction");
        assert!(
            !settings.take_failures().is_empty(),
            "Persistence errors reach the existing editor diagnostic path"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged policy"),
            "{broken"
        );
        assert_eq!(project.read("ara.project.json"), marker);
        assert!(Settings::open(&project.0, None).is_err());
    }
}
