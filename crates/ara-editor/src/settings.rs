//! Typed project settings and user preferences. Every edit is an undoable op.
//!
//! One definition table drives the Settings window, its search, `editor.settings`
//! over MCP and validation. Edits commit to a private op store (with per-author
//! undo), then persist: user scope to the preferences file chosen at open,
//! project scope to `ara.project.json` (`name`/`main` plus a `settings` object).
use ara_doc::{AssetId, Document, Edit, RecordId, Workspace};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Where a setting is stored and who shares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// Preferences for this person, on every project.
    User,
    /// Shared by everyone who opens the project (`ara.project.json`).
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
}
impl Lit {
    fn value(self) -> Value {
        match self {
            Self::B(v) => json!(v),
            Self::S(v) => json!(v),
            Self::N(v) => json!(v),
            Self::Empty => json!({}),
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
/// Every setting, grouped by category in display order.
pub const DEFS: &[Def] = &[
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
        "Scene rendering",
        "Fidelity uses the ray-traced view when the GPU supports it; Raster is the fast preview.",
        Kind::Choice(&[("fidelity", "Fidelity"), ("raster", "Raster")]),
        Lit::S("fidelity"),
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
    user_path: Option<PathBuf>,
    project_path: PathBuf,
    /// Non-default user values.
    pub user: Map<String, Value>,
    /// Non-default project values.
    pub project: Map<String, Value>,
    failures: Vec<String>,
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
        let manifest = std::fs::read_to_string(&project_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or(Value::Null);
        let mut project = manifest["settings"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (key, field) in [("project.name", "name"), ("project.mainScene", "main")] {
            if let Some(v) = manifest[field].as_str() {
                project.insert(key.into(), v.into());
            }
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
        Ok(Self {
            store: Store::new(db, ws),
            doc,
            user_path,
            project_path,
            user,
            project,
            failures: Vec::new(),
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
        let mut values = self.scope_values(d.scope).clone();
        let label = match value {
            Some(v) => {
                valid(d, &v).map_err(|e| vec![crate::invalid(e)])?;
                if v == d.default.value() {
                    values.remove(key);
                } else {
                    values.insert(key.into(), v);
                }
                format!("Set {}", d.label)
            }
            None => {
                values.remove(key);
                format!("Reset {}", d.label)
            }
        };
        if &values == self.scope_values(d.scope) {
            return Ok(label);
        }
        let mut meta = TxnMeta::by(author);
        meta.label = Some(label.clone());
        meta.why = Some(label.clone());
        let edit = Edit::Set {
            doc: self.doc.clone(),
            id: RecordId::parse(if d.scope == User { USER } else { PROJECT })
                .map_err(|e| vec![crate::invalid(e)])?,
            path: "EditorSettings.values"
                .parse::<FieldPath>()
                .map_err(|e| vec![crate::invalid(e)])?,
            value: Some(Value::Object(values)),
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
        if redo {
            self.store.redo_last(author)?;
        } else {
            self.store.undo_last(author)?;
        }
        self.reload(None);
        Ok(label)
    }
    /// Label of the edit Undo (or Redo) would revert.
    pub fn next_label(&self, author: &Author, redo: bool) -> Option<String> {
        let txn = if redo {
            let undo = self.store.next_redo(author)?;
            self.store.txn(undo.reverts.as_ref()?)?
        } else {
            self.store.next_undo(author)?
        };
        txn.label.clone()
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
        self.project = project;
        if user_changed || changed == Some(User) {
            self.persist_user();
        }
        if project_changed || changed == Some(Project) {
            self.persist_project();
        }
    }
    /// Write failures since the last call (the editor service logs them).
    pub fn take_failures(&mut self) -> Vec<String> {
        std::mem::take(&mut self.failures)
    }
    fn persist_user(&mut self) {
        let Some(path) = &self.user_path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = write_atomic(path, &Value::Object(self.user.clone())) {
            self.failures.push(format!(
                "Preferences were not saved to {}: {e}",
                path.display()
            ));
        }
    }
    fn persist_project(&mut self) {
        let mut manifest = std::fs::read_to_string(&self.project_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let mut settings = self.project.clone();
        for (key, field) in [("project.name", "name"), ("project.mainScene", "main")] {
            match settings.remove(key) {
                Some(v) => manifest[field] = v,
                None => {
                    if let Some(m) = manifest.as_object_mut() {
                        m.remove(field);
                    }
                }
            }
        }
        if let Some(m) = manifest.as_object_mut() {
            if settings.is_empty() {
                m.remove("settings");
            } else {
                m.insert("settings".into(), Value::Object(settings));
            }
        }
        if let Err(e) = write_atomic(&self.project_path, &manifest) {
            self.failures.push(format!(
                "Project settings were not saved to {}: {e}",
                self.project_path.display()
            ));
        }
    }
}
fn write_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, ara_json::to_canonical_string(value))?;
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
/// The OS location for user preferences.
pub fn user_settings_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("Ara").join("editor-settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_validate_persist_and_undo_per_author() {
        let root = std::env::temp_dir().join(format!("ara-settings-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp root");
        std::fs::write(
            root.join("ara.project.json"),
            r#"{"main":"levels/main.scene","name":"Outpost"}"#,
        )
        .expect("manifest");
        let user = root.join("user.json");
        let mut s = Settings::open(&root, Some(user.clone())).expect("open");
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
        assert_eq!(manifest["settings"]["play.tickHz"], "120");
        assert_eq!(manifest["main"], "levels/main.scene");
        assert_eq!(
            s.next_label(&Author::Human, false).as_deref(),
            Some("Set Fixed tick rate")
        );
        s.undo(&Author::Human, false).expect("undo");
        assert_eq!(s.value("play.tickHz"), json!("60"));
        s.undo(&Author::Human, true).expect("redo");
        assert_eq!(s.value("play.tickHz"), json!("120"));
        let reopened = Settings::open(&root, Some(user)).expect("reopen");
        assert_eq!(reopened.value("appearance.theme"), json!("light"));
        let _ = std::fs::remove_dir_all(root);
    }
}
