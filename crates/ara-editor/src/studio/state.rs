//! The studio runtime's state: the [`Editor`] (the authority, navigation state and command
//! surface agents share), the scene as resolved at the current revision, the look, and what
//! only the window keeps (open menus, the palette, drags, layout rects the host feeds back).
use std::collections::HashMap;
use std::sync::Arc;

use ara_doc::prefab::Resolved;
use ara_kit::look::Look;
use serde_json::{Value, json};
use stillkit::Rect;

use crate::Editor;

/// What the window adds around its drawn content.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    /// The macOS traffic lights' right edge, design px (0 elsewhere).
    pub inset: f32,
    /// Drawn minimize/maximize/close, and whether maximized (`None` on macOS).
    pub controls: Option<bool>,
}

/// A splitter being dragged: its dock path and the ratio when the drag began.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitDrag {
    /// Path of the split in the dock tree (`0` first, `1` second at each level).
    pub path: Vec<u8>,
    /// Whether the split lays its children side by side.
    pub horizontal: bool,
}

/// The Scene view's renderer status, as the host last saw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneStatus {
    /// The renderer is ready and the view shows frames.
    pub ready: bool,
    /// Why it isn't, or what it is doing ("Preparing the scene renderer…").
    pub message: String,
    /// Smoothed frames per second and frame time in ms, while frames run.
    pub fps: Option<(f32, f32)>,
}

/// One diagnostics-bus record, as the Console and Problems show it.
#[derive(Clone, Debug)]
pub struct LogRow {
    /// Bus sequence number.
    pub seq: u64,
    /// Error, warning or info.
    pub severity: ara_diag::Severity,
    /// The catalog code (`ARA0042` or its label).
    pub code: String,
    /// The message.
    pub message: String,
    /// Who reported it (`engine`, `script`, an agent).
    pub origin: String,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    /// `file:line`, or the document, when the record names one.
    pub source: Option<String>,
    /// Repetitions merged into this record.
    pub count: u64,
    /// The whole diagnostic: its location and typed fixes.
    pub diagnostic: Arc<ara_diag::Diagnostic>,
}

impl LogRow {
    /// A row from a `logs` entry; `None` when the entry is malformed.
    pub fn from_entry(e: &Value) -> Option<Self> {
        let d: ara_diag::Diagnostic = serde_json::from_value(e["diagnostic"].clone()).ok()?;
        let source = match (&d.location.file, d.location.line, &d.location.doc) {
            (Some(f), Some(line), _) => Some(format!("{f}:{line}")),
            (Some(f), None, _) => Some(f.clone()),
            (None, _, Some(doc)) => Some(doc.clone()),
            _ => None,
        };
        Some(Self {
            seq: e["seq"].as_u64()?,
            severity: d.severity,
            code: d.code.to_string(),
            message: d.message.clone(),
            origin: e["origin"].as_str().unwrap_or("engine").to_owned(),
            at: e["at"].as_u64().unwrap_or(0),
            source,
            count: e["count"].as_u64().unwrap_or(1),
            diagnostic: Arc::new(d),
        })
    }
}

/// A Hierarchy row drag: what moves, and where it would land.
#[derive(Clone, Debug, PartialEq)]
pub struct RowDrag {
    /// The pressed row's entity (the selection travels with it when it is selected).
    pub source: String,
    /// The pressed row's index among the shown rows.
    pub at: usize,
    /// The row under the pointer and where relative to it, once the drag has moved.
    pub target: Option<(String, ara_ui::drag_drop::Placement)>,
}

/// Where the pointer rests over the Scene view (for Shift+A at the pointer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScenePointer {
    /// From the view's top-left, view px.
    pub at: [f32; 2],
    /// The view's size, view px.
    pub size: (f32, f32),
    /// In the window, design px.
    pub window: (f32, f32),
}

/// Asset tile pictures: the legacy pipeline's off-thread renders (cached by content under
/// `.ara/cache/thumbnails`), turned into stillkit images as they land.
#[derive(Default)]
pub(crate) struct Previews {
    /// The render queue, made on first use.
    queue: Option<crate::asset_tools::thumbnails::Thumbnails>,
    /// Paths already asked for.
    asked: std::collections::HashSet<String>,
    /// Finished pictures, by storage path.
    pub images: std::collections::HashMap<String, std::sync::Arc<stillkit::Image>>,
    /// Some sources had not finished importing; they are asked again each second.
    waiting: bool,
    /// When they were last asked again.
    retried: Option<std::time::Instant>,
}

impl Previews {
    /// Whether a file of `kind` has a picture worth rendering.
    pub fn previewable(kind: &str) -> bool {
        matches!(
            kind,
            "png" | "jpg" | "jpeg" | "hdr" | "exr" | "glb" | "gltf" | "fbx" | "material" | "graph"
        )
    }

    /// Asks for `paths` not asked for yet.
    pub fn want<'a>(&mut self, root: &std::path::Path, paths: impl IntoIterator<Item = &'a str>) {
        for path in paths {
            if self.asked.insert(path.to_owned()) {
                self.queue
                    .get_or_insert_with(|| crate::asset_tools::thumbnails::Thumbnails::new(root))
                    .request(path);
            }
        }
    }

    /// Takes finished pictures; whether any landed.
    pub fn poll(&mut self) -> bool {
        let Some(queue) = self.queue.as_mut() else {
            return false;
        };
        if self.waiting
            && self
                .retried
                .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(1))
        {
            self.retried = Some(std::time::Instant::now());
            self.waiting = queue.retry_pending();
        }
        let mut landed = false;
        for done in queue.poll() {
            if let crate::asset_tools::thumbnails::Done::Pending(_) = &done {
                self.waiting = true;
            }
            if let crate::asset_tools::thumbnails::Done::Image(thumb) = done {
                let (width, height) = thumb.image.dimensions();
                let mut pixels = Vec::with_capacity((width * height * 4) as usize);
                for p in thumb.image.pixels() {
                    let a = u16::from(p[3]);
                    let pre = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
                    pixels.extend_from_slice(&[pre(p[2]), pre(p[1]), pre(p[0]), p[3]]);
                }
                self.images.insert(
                    thumb.key,
                    std::sync::Arc::new(stillkit::Image {
                        width,
                        height,
                        pixels,
                    }),
                );
                landed = true;
            }
        }
        landed
    }
}

impl std::fmt::Debug for Previews {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Previews")
            .field("images", &self.images.len())
            .finish()
    }
}

/// Window-only state.
#[derive(Debug, Default)]
pub struct Local {
    /// The open title-bar menu, by its name.
    pub menu: Option<&'static str>,
    /// The command palette: open, query, highlighted result.
    pub palette: Option<(String, usize)>,
    /// Laid-out rects of keyed elements the host reports (splits, the Scene view).
    pub rects: HashMap<String, Rect>,
    /// The splitter being dragged.
    pub drag: Option<SplitDrag>,
    /// The entity being renamed in the hierarchy, and its draft name.
    pub rename: Option<(String, String)>,
    /// Inspector components folded shut, by component name.
    pub folded: std::collections::BTreeSet<String>,
    /// The field being typed into, and its draft text.
    pub editing: Option<(String, String)>,
    /// The Scene view's renderer status.
    pub scene: SceneStatus,
    /// Console rows shown: errors, warnings, info.
    pub console: [bool; 3],
    /// A click in the Scene view the host resolves against the renderer.
    pub pick: Option<super::scene::Pick>,
    /// The hierarchy's search.
    pub hierarchy_query: String,
    /// A number being scrubbed: field path, channel, value at the press, and its value now.
    pub scrub: Option<(String, Option<usize>, f64, f64)>,
    /// The open dropdown, by key.
    pub select: Option<String>,
    /// The open context menu: its layout, what it acts on, and where (window design px).
    pub(crate) context: Option<(
        crate::shell::commands::menus::Context,
        crate::shell::commands::Target,
        (f32, f32),
    )>,
    /// An element to focus once the view shows it (a command that opened a field).
    pub focus: Option<&'static str>,
    /// The open field menu: the field's path, its value as JSON, and where (window design px).
    pub field_menu: Option<(String, String, (f32, f32))>,
    /// Text for the host to put on the system clipboard.
    pub copy: Option<String>,
    /// The open Settings window.
    pub settings: Option<super::preferences::View>,
    /// The Console row shown in the detail pane, by bus sequence number.
    pub console_open: Option<u64>,
    /// Commands run from the palette lately, newest first (never destructive ones).
    pub recent_commands: Vec<&'static str>,
    /// Scale fields keep their proportions (the Inspector's chain toggle).
    pub scale_linked: bool,
    /// The open component overflow menu, by component.
    pub component_menu: Option<String>,
    /// Add Component's search and highlighted row, while open.
    pub add_component: Option<(String, usize)>,
    /// An asset being dragged toward the Scene: its storage path, name, and where the
    /// pointer is (window design px).
    pub asset_drag: Option<(String, String, (f32, f32))>,
    /// A Hierarchy row being dragged.
    pub row_drag: Option<RowDrag>,
    /// Agent turns shown open in Agents, by turn key.
    pub expanded_turns: std::collections::BTreeSet<String>,
    /// The open Create browser.
    pub create: Option<super::create::Browser>,
    /// What was created lately, newest first (the browser's Recent).
    pub recent: Vec<&'static str>,
    /// The pointer over the Scene view.
    pub scene_pointer: Option<ScenePointer>,
    /// The gizmo handle being dragged.
    pub gizmo: Option<super::gizmo::Drag>,
    /// The right button is held in the Scene view: WASD/QE fly the camera.
    pub flying: bool,
    /// Fly keys held down (W A S D Q E, as virtual keys).
    pub held: std::collections::BTreeSet<u16>,
    /// The gizmo handle under the pointer.
    pub gizmo_hover: Option<crate::shell::overlay::Handle>,
}

/// The studio's runtime state.
pub struct Studio {
    /// The editor: authority, navigation state, actions.
    pub editor: Editor,
    /// The current scheme's tokens and theme.
    pub look: Look,
    /// The open scene's entities at the current revision.
    pub resolved: Arc<Resolved>,
    /// Window controls.
    pub frame: Frame,
    /// Window-only state.
    pub local: Local,
    /// The window's size in design px.
    pub size: (f32, f32),
    /// The project's files, scanned in the background (Assets).
    pub(crate) assets: Option<crate::shell::assets::Browser>,
    /// Asset thumbnails rendered off-thread (models, materials, textures), by storage path.
    pub(crate) previews: Previews,
    /// The diagnostics bus so far (Console, Problems), oldest first.
    pub logs: Vec<LogRow>,
}

impl Studio {
    /// A studio over an open editor.
    pub fn new(editor: Editor, size: (f32, f32)) -> Self {
        let look = Look::new(!editor.state.light);
        let mut studio = Self {
            editor,
            look,
            resolved: Arc::default(),
            frame: Frame::default(),
            local: Local {
                console: [true; 3],
                ..Local::default()
            },
            size,
            assets: None,
            previews: Previews::default(),
            logs: Vec::new(),
        };
        studio.resolve();
        studio
    }

    /// Resolves the open scene again (after a committed change).
    pub fn resolve(&mut self) {
        self.resolved = crate::world::resolve(self.editor.host.store(), &self.editor.state.doc)
            .map(Arc::new)
            .unwrap_or_default();
    }

    /// Runs an editor action as the window's person; failures are reported like any
    /// other editor failure (Console, Problems) rather than lost.
    pub fn act(&mut self, method: &str, params: Value) {
        let result = super::trace::time(method, std::time::Duration::from_millis(5), || {
            self.editor.action(method, params)
        });
        if let Err(error) = result {
            self.editor.report_failure(&error);
        }
    }

    /// Commits content commands (one undo step) as the window's person.
    pub fn apply(&mut self, commands: Vec<Value>, why: &str) {
        match super::trace::time(why, std::time::Duration::from_millis(5), || {
            self.editor.apply(commands, why, None)
        }) {
            Ok(_) => self.resolve(),
            Err(error) => self.editor.report_failure(&error),
        }
    }

    /// Selects `entities` (replacing, or toggling one with `toggle`).
    pub fn select(&mut self, entities: Vec<String>, toggle: bool) {
        self.act(
            "editor.select",
            json!({"entities": entities, "toggle": toggle}),
        );
    }

    /// The look follows the editor's theme preference; whether it changed.
    pub fn sync_look(&mut self) -> bool {
        let dark = !self.editor.state.light;
        if self.look.dark == dark {
            return false;
        }
        self.look = Look::new(dark);
        true
    }

    /// Reads new bus records into [`Self::logs`] (bounded to the last 2000); whether any came.
    pub fn poll_logs(&mut self) -> bool {
        let since = self.logs.last().map_or(0, |r| r.seq);
        let Ok(logs) = self.editor.action(
            "logs",
            json!({"group": false, "limit": 500, "since": since}),
        ) else {
            return false;
        };
        let before = self.logs.len();
        self.logs.extend(
            logs["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(LogRow::from_entry)
                .filter(|r| r.seq > since),
        );
        let excess = self.logs.len().saturating_sub(2000);
        self.logs.drain(..excess);
        self.logs.len() != before || excess > 0
    }

    /// An entity's display name.
    pub fn name(&self, id: &str) -> String {
        self.resolved
            .get(id)
            .and_then(|e| e.record.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned()
    }
}
