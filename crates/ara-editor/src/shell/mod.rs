//! Retained native panel composition and semantic interaction helpers.
//!
//! The main window and each floating window are separate retained `Ui` trees
//! ("surfaces"). Panel code always works on `self.ui`; [`Shell::enter`] swaps a
//! floating surface in and [`Shell::leave`] swaps it back, so panels need not
//! know which OS window hosts them.
mod assets;
mod axis;
pub(crate) mod build;
mod chrome;
pub mod commands;
mod compound;
mod dialog;
mod drag;
pub(crate) mod icons;
mod imports;
mod input;
mod keyboard;
mod menu;
mod motion;
mod notices;
mod ops;
mod polish;
pub(crate) mod scripts;
mod scrub;
mod settings_panel;
mod testing;
pub(crate) use drag::Drag;
pub use testing::MenuRow;
mod agents_panel;
mod animation_panel;
mod assets_panel;
mod game_panel;
mod graph_annotations;
mod graph_input;
mod graph_panel;
mod hierarchy;
mod import_panel;
mod inspector_panel;
mod log_panels;
mod material_panel;
mod memo;
mod names;
pub(crate) use animation_panel::LiveChange;
pub(crate) use log_panels::LogView;
mod material_inspector;
pub mod overlay;
mod palette;
mod panels;
pub mod prefab_ops;
mod profiler_panel;
mod vfx_panel;
use crate::{Editor, inspector::Field};
use ara_ui::{
    Ui,
    input::InputState,
    paint::Box2,
    virtualize::{VirtualList, VirtualView},
};
use ara_ui_text::TextSystem;
pub(crate) use dialog::Dialog;
pub(crate) use game_panel::game_stats_text;
pub use hierarchy::LARGE_GROUP;
pub(crate) use ops::{WindowRequest, open_docs, open_external, open_terminal};
use panels::{button, escape};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A floating OS window's retained UI, swapped into `Shell::ui` while it is handled.
pub struct Surface {
    /// Floating window id from the layout.
    pub id: String,
    /// Retained runtime tree and display list.
    pub ui: Ui,
    /// The window's own focus and capture.
    pub input: InputState,
    /// Pointer in this window's logical pixels.
    pub pointer: [f32; 2],
}
/// Native shell and its retained widget state; no authoring document is mirrored here.
pub struct Shell {
    /// Native runtime tree and renderer-neutral display list of the current surface.
    pub ui: Ui,
    /// Explicit bundled fonts and retained shaping contexts.
    pub text: TextSystem,
    /// Native focus, capture and keyboard navigation of the current surface.
    pub input: InputState,
    /// Floating windows; the current one (if any) is swapped into `ui`.
    pub floats: Vec<Surface>,
    current: Option<String>,
    /// Active schema field models keyed by widget identity.
    pub fields: BTreeMap<String, Field>,
    /// Pointer location in logical pixels of the current surface.
    pub pointer: [f32; 2],
    /// Shift/control/alt modifiers, updated from physical keys.
    pub modifiers: [bool; 3],
    /// Current tree scroll offset, independent of the finite widget pool.
    pub hierarchy_offset: f32,
    pub(crate) bindings: BTreeMap<u64, Binding>,
    hierarchy: VirtualList,
    hierarchy_entities: BTreeMap<u64, String>,
    hierarchy_tints: BTreeMap<u64, &'static str>,
    /// What the Hierarchy rows were last built from.
    hierarchy_key: Option<hierarchy::HierarchyKey>,
    /// The store revision `hierarchy_structure` last checked, and the revision of the
    /// last change the Hierarchy shows.
    hierarchy_revisions: (u64, u64),
    /// Rebuilds are held (`coalesce_rebuilds`), and whether one was asked for meanwhile.
    hold_rebuilds: bool,
    held_rebuild: bool,
    /// Entities in the open scene (the Hierarchy may show fewer rows: collapsed groups).
    entity_count: usize,
    hierarchy_ghosts: std::collections::BTreeSet<u64>,
    /// Game view: players of the shown world, the selected one, whether a frame is
    /// showing, frames per second, the surface size, the wanted size (physical px),
    /// input capture, aspect preset and the stats overlay.
    pub(crate) game_players: Vec<usize>,
    pub(crate) game_client: usize,
    pub(crate) game_live: bool,
    pub(crate) game_fps: u32,
    pub(crate) game_size: [u32; 2],
    pub(crate) game_target: [u32; 2],
    pub(crate) game_captured: bool,
    pub(crate) game_aspect: &'static str,
    pub(crate) game_stats: bool,
    /// Outside Play: the gameplay camera's preview image size (physical px) once one
    /// is shown (`app/game_preview.rs`), and the camera by `resolved_key`.
    pub(crate) game_preview: Option<[u32; 2]>,
    game_camera: Option<((String, u64, usize), Option<crate::state::Camera>)>,
    /// The shown Play world's fault (`play.status`), shown instead of the game.
    pub(crate) game_fault: Option<String>,
    /// The shown Play world is still starting (`play.status`): its progress.
    pub(crate) game_start: Option<game_panel::StartProgress>,
    /// `game_fault` is a start that failed, not a fault while running.
    pub(crate) game_start_failed: bool,
    /// The shown Play world's last `play.status` said `running`. Live reads of the
    /// world (`play.read`) wait for it: a starting or stopping world has none yet.
    pub(crate) game_running: bool,
    /// The Material tool shows its DSL source instead of the node graph.
    pub(crate) material_source_view: bool,
    /// In-node material previews (requests and the show/hide toggle).
    pub(crate) node_previews: crate::asset_tools::node_preview::NodePreviews,
    /// The running animator the Animation page follows during Play.
    pub(crate) animation_live: animation_panel::AnimationLive,
    /// Colour swatches outside the Inspector (material parameters): node id and
    /// colour, painted after layout like the Inspector's.
    value_swatches: Vec<(String, ara_ui::style::Color)>,
    /// Colour swatches of compound Inspector elements: control ID and linear value.
    nested_swatches: Vec<(String, Value)>,
    /// Create > C# Behaviour opens the IDE (verification routes turn it off).
    pub(crate) open_ide: bool,
    /// The running export (Build > Build Current Profile).
    pub(crate) build: Option<build::BuildJob>,
    /// The last finished build's game folder (Build > Run Build, Open Build Folder).
    pub(crate) last_build: Option<std::path::PathBuf>,
    /// A running headless Run Build (offscreen runs).
    pub(crate) run: Option<std::sync::mpsc::Receiver<build::RunResult>>,
    /// The capture the last headless Run Build wrote.
    pub(crate) last_run: Option<std::path::PathBuf>,
    /// The window is hidden (captures, verification): Run Build stays headless and
    /// nothing opens on the desktop.
    pub(crate) offscreen: bool,
    /// Assets lists every file, including glTF buffers and project files.
    pub(crate) assets_show_all: bool,
    /// Console and Problems filters.
    pub(crate) log_view: LogView,
    /// Console row whose details are expanded (bus sequence number).
    pub(crate) console_open: Option<u64>,
    /// Whether Console and Problems show this computer's GPU environment messages.
    pub(crate) machine_open: bool,
    /// Profiler scope outlined in the timeline (picked in the Scopes table).
    pub(crate) profile_scope: Option<String>,
    /// Problems whose "Why" explanation is expanded.
    pub(crate) explained_problems: std::collections::BTreeSet<u64>,
    /// Failures found while polling (thumbnails), logged by the window owner.
    pub(crate) pending_reports: Vec<(&'static str, String)>,
    /// Agent turns whose changes the activity feed shows expanded.
    pub(crate) expanded_turns: std::collections::BTreeSet<String>,
    hierarchy_view: Option<VirtualView>,
    /// Surface hosting the hierarchy's virtual rows (`None` is the main window).
    hierarchy_surface: Option<String>,
    browser: assets::Browser,
    /// The asset pipeline's import progress (status chip, Assets badges, Scene notice).
    imports: imports::Imports,
    asset_rows: names::AssetRows,
    /// Caption lines of Assets tile names (fitted once per name and width).
    name_fit: names::NameFit,
    asset_entries: BTreeMap<u64, assets::Asset>,
    asset_view: Option<VirtualView>,
    asset_surface: Option<String>,
    asset_offset: f32,
    thumbnails: crate::asset_tools::thumbnails::Thumbnails,
    thumbnail_slots: BTreeMap<String, usize>,
    /// Bumped whenever `thumbnail_slots` changes (memoized panels show thumbnails).
    thumbnail_revision: u64,
    /// Memoized panels' markup and bindings, by panel (`memo`).
    memos: BTreeMap<&'static str, memo::Memo>,
    /// Regenerate every reused memoized panel and compare (`ARA_EDITOR_MEMO_CHECK=1`,
    /// set for `cargo test`): a difference means a digest misses an input.
    pub memo_check: bool,
    /// Reuse memoized panels at all (`ARA_EDITOR_MEMO=0` turns it off: to measure the
    /// saving, or to rule out a stale panel).
    memo_enabled: bool,
    thumbnail_next: usize,
    thumbnail_errors: BTreeMap<String, String>,
    material_previews: BTreeMap<String, String>,
    material_preview_key: Option<String>,
    material_sources: BTreeMap<String, String>,
    thumbnail_uploads: Vec<(u32, ara_ui_render::gpu::Image)>,
    asset_grid_extent: f32,
    asset_grid_size: [f32; 2],
    asset_grid_dirty: bool,
    drafts: BTreeMap<String, String>,
    /// The dock areas' headers and tab bodies from the markup being built (host ID,
    /// part), mounted with `Ui::reconcile_parts`.
    parts: Vec<(String, String)>,
    /// Tab bodies in `parts` that are mounted but hidden (inactive tabs).
    hidden_bodies: Vec<String>,
    /// Each `panels::keep_alive` panel's last body markup, mounted again unchanged
    /// while its tab is inactive so its retained subtree stays as it is.
    bodies: BTreeMap<String, String>,
    pub(crate) palette_command: Option<String>,
    pub(crate) palette_params: BTreeMap<String, Value>,
    palette_active: usize,
    pub(crate) pick_field: Option<String>,
    pub(crate) rename: bool,
    pub(crate) drag: Option<Drag>,
    scrub: Option<scrub::Scrub>,
    motion: motion::Motion,
    reduce_motion: bool,
    panel_opened: Option<std::time::Instant>,
    color_picker: Option<String>,
    modal_open: bool,
    canvas_modal_open: bool,
    collapsed_components: std::collections::BTreeSet<String>,
    hierarchy_style: BTreeMap<u64, polish::RowStyle>,
    selected_rows: std::collections::BTreeSet<u64>,
    pub(crate) version: u64,
    /// UI work since the host last took it (`take_stats`).
    pub(crate) stats: UiStats,
    /// The editor's panels and store revisions the last rebuild reflected, so a
    /// revision the shell already rebuilt for does not rebuild it again.
    built: (u64, u64),
    /// The open scene resolved, with the (document, store revision, document count)
    /// it was resolved at (`resolved`).
    resolved_cache: Option<(
        (String, u64, usize),
        std::sync::Arc<ara_doc::prefab::Resolved>,
    )>,
    tooltip_text: Option<String>,
    field_request: Option<String>,
    /// Each graph page's canvas (`graph_panel::GRAPH_PAGES`).
    graphs: BTreeMap<&'static str, graph_panel::GraphCanvas>,
    /// The graph page the navigation state, graph commands and keys act on.
    graph_page: &'static str,
    canvas_display: ara_ui::paint::DisplayList,
    canvas_navigation: u64,
    canvas_dragging: bool,
    resource_text: Option<(String, String)>,
    vfx_stream: crate::asset_tools::vfx_stream::VfxStream,
    vfx_visible: bool,
    vfx_ready: bool,
    vfx_error: Option<String>,
    /// VFX timeline: preview rate (percent), the effect's tick rate and the latest
    /// GPU counters.
    pub(crate) vfx_rate: u32,
    vfx_hz: u32,
    vfx_live: Option<vfx_panel::VfxLive>,
    /// Preview-only emitter state: hidden, isolated (solo) and collapsed in the stack.
    pub(crate) vfx_hidden: std::collections::BTreeSet<String>,
    pub(crate) vfx_solo: Option<String>,
    pub(crate) vfx_collapsed: std::collections::BTreeSet<String>,
    axis_camera: crate::state::Camera,
    /// Open menu bar menu, context menu or value menu.
    pub(crate) menu: Option<menu::MenuState>,
    menu_anchors: Vec<f32>,
    mnemonics: bool,
    alt_tap: bool,
    /// Modal information or input dialog.
    pub(crate) dialog: Option<Dialog>,
    /// OS window work for the window owner (full screen, raise, import dialog, quit).
    pub window_request: Option<WindowRequest>,
    pub(crate) status_note: Option<String>,
    /// GPU, driver and Vulkan summary for Help › System Info.
    pub device_info: String,
    pub(crate) entity_clipboard: Option<Vec<Value>>,
    pub(crate) field_clipboard: Option<Value>,
    pending_clipboard: Option<String>,
    recent_commands: Vec<&'static str>,
    /// Command waiting for a new shortcut on the Keyboard Shortcuts page.
    pub(crate) recording: Option<&'static str>,
    key_search: bool,
    label_width: f32,
    keymap: commands::keys::Keymap,
    /// Bumped whenever `keymap` is rebuilt (memoized panels show shortcuts).
    keymap_revision: u64,
    /// The settings `keymap` was built from (`input.keymap`, `input.shortcuts`).
    keymap_settings: (Option<Value>, Option<Value>),
    /// Live tab drag target, painted as a lit region with an action label.
    pub(crate) drop_preview: Option<drag::DropPreview>,
    /// The desktop pointer (points) cross-window tab drags follow; `None` reads the
    /// OS. Verification routes set it to drive those drags with synthetic events.
    pub(crate) desktop_pointer: Option<[f32; 2]>,

    /// Main window's desktop rectangle (x, y, width, height), kept by the window owner.
    pub main_rect: [i32; 4],
}
/// UI cost counters a host reads per frame (`Shell::take_stats`).
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct UiStats {
    /// Full shell rebuilds (markup regenerated and mounted).
    pub rebuilds: u32,
    /// Milliseconds spent in rebuilds.
    pub rebuild_ms: f64,
    /// Of `rebuild_ms`: mounting (reconcile, virtual rows, frames); the rest is markup.
    pub mount_ms: f64,
    /// Of `mount_ms`: `Ui::reconcile` alone.
    pub reconcile_ms: f64,
    /// Of `mount_ms`: the mount's layout frame (restyle, layout, paint).
    pub frame_ms: f64,
    /// Of `mount_ms`: mounting and filling the virtual row pools (Hierarchy, Assets).
    pub rows_ms: f64,
    /// Runtime work summed over every `Ui::frame` (rebuild mounts and the frame's own).
    pub work: ara_ui::Work,
}
#[derive(Clone, PartialEq)]
pub(crate) enum Binding {
    Protocol(&'static str, Value),
    Command(&'static str, commands::Target),
    Content(Vec<Value>, String),
    Field(String, Option<usize>),
    /// The Scene toolbar's field-of-view field (drafted, applied on commit).
    CameraFov,
    Search(String),
    Palette(String),
    PaletteParam(String),
    PaletteRun,
    Select(String),
    Asset(assets::Asset),
    ResourceContent(String, Vec<Value>, String),
    ImportField(String, String),
    ImportChoice(String, String, Value),
    ResourceText(String, String),
    ResourceApply,
    PreviewShape(bool),
    VfxPlayback(bool),
    AnimationEdit(String),
    AnimationField(String, String, String),
    VfxField(String, String, String),
    MaterialValue(String, String),
    GraphLabel(String, String),
    Fix(ara_diag::Diagnostic, usize),
    PickField(String),
    /// An asset field's picker popup: the field's path and declared asset kind.
    AssetPicker(String, Option<String>),
    /// A typed text box inside a compound field: its `path#pointer` target, how its
    /// text parses, and whether empty text clears it.
    Nested(String, compound::Leaf, bool),
    RenameValue,
    ColorPicker(String),
    ComponentCollapse(String),
    Scrub(String, usize),
    Diagnostic(ara_diag::Diagnostic),
    MenuBar(usize),
    MenuSub(usize, usize),
    MenuFilter,
    ContextMenu(commands::menus::Context, commands::Target),
    EnumMenu(String, Vec<String>),
    Setting(String),
    SettingChoice(String, Value),
    ShortcutRecord(&'static str),
    ShortcutReset(&'static str),
    KeySearch,
    DialogText,
    DialogAction(&'static str),
}
/// The editor's fonts: Inter (UI), JetBrains Mono (code, timings) and Noto Naskh
/// Arabic (right-to-left text). The shell and the loading window share them.
pub(crate) fn text_system() -> ara_ui::Result<TextSystem> {
    TextSystem::new(
        [
            include_bytes!("../../assets/fonts/InterVariable.ttf").to_vec(),
            include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").to_vec(),
            include_bytes!("../../../ara-ui-text/fonts/NotoNaskhArabic.ttf").to_vec(),
        ],
        "'Inter Variable', 'Noto Naskh Arabic'",
    )
}
impl Shell {
    /// Build the native shell with the project's initial layout and model snapshots.
    pub fn new(editor: &mut Editor, size: [f32; 2]) -> ara_ui::Result<Self> {
        let text = text_system()?;
        let ui = Ui::editor("<ui id='editor' />", include_str!("editor.css"), size)?;
        let mut shell = Self {
            ui,
            text,
            input: InputState::default(),
            floats: Vec::new(),
            current: None,
            fields: BTreeMap::new(),
            pointer: [0.; 2],
            modifiers: [false; 3],
            hierarchy_offset: 0.,
            bindings: BTreeMap::new(),
            hierarchy: VirtualList { items: Vec::new() },
            hierarchy_entities: BTreeMap::new(),
            hierarchy_tints: BTreeMap::new(),
            hierarchy_key: None,
            hierarchy_revisions: (0, 0),
            hold_rebuilds: false,
            held_rebuild: false,
            entity_count: 0,
            hierarchy_ghosts: std::collections::BTreeSet::new(),
            expanded_turns: std::collections::BTreeSet::new(),
            pending_reports: Vec::new(),
            log_view: LogView::default(),
            assets_show_all: false,
            build: None,
            last_build: None,
            run: None,
            last_run: None,
            offscreen: false,
            open_ide: true,
            game_players: Vec::new(),
            game_client: 0,
            game_live: false,
            game_fps: 0,
            game_size: [0, 0],
            game_target: [0, 0],
            game_captured: false,
            game_aspect: "free",
            game_preview: None,
            game_camera: None,
            game_stats: true,
            game_fault: None,
            game_start: None,
            game_start_failed: false,
            game_running: false,
            material_source_view: false,
            node_previews: Default::default(),
            animation_live: Default::default(),
            value_swatches: Vec::new(),
            nested_swatches: Vec::new(),
            console_open: None,
            machine_open: false,
            profile_scope: None,
            explained_problems: std::collections::BTreeSet::new(),
            hierarchy_view: None,
            hierarchy_surface: None,
            browser: assets::Browser::new(editor.host.root()),
            imports: imports::Imports::new(editor.host.root()),
            asset_rows: names::AssetRows::default(),
            name_fit: names::NameFit::default(),
            asset_entries: BTreeMap::new(),
            asset_view: None,
            asset_surface: None,
            asset_offset: 0.,
            thumbnails: crate::asset_tools::thumbnails::Thumbnails::new(editor.host.root()),
            thumbnail_slots: BTreeMap::new(),
            thumbnail_revision: 0,
            memos: BTreeMap::new(),
            memo_check: std::env::var_os("ARA_EDITOR_MEMO_CHECK").is_some_and(|v| v == "1"),
            memo_enabled: std::env::var_os("ARA_EDITOR_MEMO").is_none_or(|v| v != "0"),
            thumbnail_next: 0,
            thumbnail_errors: BTreeMap::new(),
            material_previews: BTreeMap::new(),
            material_preview_key: None,
            material_sources: BTreeMap::new(),
            thumbnail_uploads: Vec::new(),
            asset_grid_extent: 0.,
            asset_grid_size: [0.; 2],
            asset_grid_dirty: false,
            drafts: BTreeMap::new(),
            parts: Vec::new(),
            hidden_bodies: Vec::new(),
            bodies: BTreeMap::new(),
            palette_command: None,
            palette_params: BTreeMap::new(),
            palette_active: 0,
            pick_field: None,
            rename: false,
            drag: None,
            scrub: None,
            motion: Default::default(),
            reduce_motion: false,
            panel_opened: None,
            color_picker: None,
            modal_open: false,
            canvas_modal_open: false,
            collapsed_components: Default::default(),
            hierarchy_style: BTreeMap::new(),
            selected_rows: Default::default(),
            version: 0,
            stats: UiStats::default(),
            built: (u64::MAX, u64::MAX),
            resolved_cache: None,
            tooltip_text: None,
            field_request: None,
            canvas_display: Default::default(),
            graphs: BTreeMap::new(),
            graph_page: graph_panel::GRAPH_PAGES[0],
            canvas_navigation: 0,
            canvas_dragging: false,
            resource_text: None,
            vfx_stream: crate::asset_tools::vfx_stream::VfxStream::new(editor.host.root()),
            vfx_visible: false,
            vfx_ready: false,
            vfx_error: None,
            vfx_rate: 100,
            vfx_hz: 60,
            vfx_live: None,
            vfx_hidden: std::collections::BTreeSet::new(),
            vfx_solo: None,
            vfx_collapsed: std::collections::BTreeSet::new(),
            axis_camera: Default::default(),
            menu: None,
            menu_anchors: Vec::new(),
            mnemonics: false,
            alt_tap: false,
            dialog: None,
            window_request: None,
            status_note: None,
            device_info: String::new(),
            entity_clipboard: None,
            field_clipboard: None,
            pending_clipboard: None,
            recent_commands: Vec::new(),
            recording: None,
            key_search: false,
            label_width: 0.45,
            keymap: commands::keymap(&editor.state),
            keymap_revision: 0,
            keymap_settings: keymap_settings(&editor.state),
            drop_preview: None,
            desktop_pointer: None,
            main_rect: [0, 0, size[0] as i32, size[1] as i32],
        };
        shell.rebuild(editor)?;
        Ok(shell)
    }
    /// Swap a floating surface into `ui`/`input`/`pointer`; false if it does not exist.
    pub fn enter(&mut self, id: &str) -> bool {
        if self.current.as_deref() == Some(id) {
            return true;
        }
        self.leave();
        let Some(index) = self.floats.iter().position(|s| s.id == id) else {
            return false;
        };
        let surface = &mut self.floats[index];
        std::mem::swap(&mut self.ui, &mut surface.ui);
        std::mem::swap(&mut self.input, &mut surface.input);
        std::mem::swap(&mut self.pointer, &mut surface.pointer);
        self.current = Some(id.to_owned());
        true
    }
    /// Swap the current floating surface back; the main window is current afterwards.
    pub fn leave(&mut self) {
        let Some(id) = self.current.take() else {
            return;
        };
        if let Some(surface) = self.floats.iter_mut().find(|s| s.id == id) {
            std::mem::swap(&mut self.ui, &mut surface.ui);
            std::mem::swap(&mut self.input, &mut surface.input);
            std::mem::swap(&mut self.pointer, &mut surface.pointer);
        }
    }
    /// The floating window currently swapped in, if any.
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }
    /// Reconcile a document/navigation notification. Unchanged idle frames never rebuild panels.
    /// Inside [`Self::coalesce_rebuilds`] it only notes that a rebuild is due.
    pub fn rebuild(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.hold_rebuilds {
            self.held_rebuild = true;
            return Ok(());
        }
        let started = std::time::Instant::now();
        let returning = self.current.clone();
        self.leave();
        let result = self.rebuild_all(editor);
        if let Some(id) = returning {
            self.enter(&id);
        }
        self.stats.rebuilds += 1;
        self.stats.rebuild_ms += started.elapsed().as_secs_f64() * 1000.;
        result
    }
    /// The open scene with prefabs resolved, reused by every panel until the store
    /// changes: a transaction (the revision), or a document loaded or unloaded (the
    /// count).
    pub(crate) fn resolved(
        &mut self,
        editor: &Editor,
    ) -> ara_ui::Result<std::sync::Arc<ara_doc::prefab::Resolved>> {
        let store = editor.host.store();
        let key = (
            editor.state.doc.clone(),
            store.revision(),
            store.workspace().docs().count(),
        );
        if let Some((cached, resolved)) = &self.resolved_cache
            && *cached == key
        {
            return Ok(resolved.clone());
        }
        let resolved = std::sync::Arc::new(
            crate::world::resolve(store, &editor.state.doc).map_err(|d| format!("{d:?}"))?,
        );
        self.resolved_cache = Some((key, resolved.clone()));
        Ok(resolved)
    }
    /// The open scene's view at the current revision, built on [`Self::resolved`] so
    /// a commit resolves the scene once for the panels and the Scene view.
    pub(crate) fn document_view(
        &mut self,
        editor: &Editor,
    ) -> ara_ui::Result<crate::world::DocumentView> {
        let resolved = self.resolved(editor)?;
        Ok(crate::world::DocumentView::with_entities(
            editor.host.store(),
            resolved,
        ))
    }
    /// The (document, store revision, document count) `resolved` last resolved at.
    pub(crate) fn resolved_key(&self) -> Option<(String, u64, usize)> {
        self.resolved_cache.as_ref().map(|(key, _)| key.clone())
    }
    /// Uncommitted field text, keyed by field (`path#axis`, or `@…` for other panels).
    pub(crate) fn drafts(&self) -> &BTreeMap<String, String> {
        &self.drafts
    }
    /// The UI work done since the last call: rebuilds and every runtime frame's counters.
    pub fn take_stats(&mut self) -> UiStats {
        std::mem::take(&mut self.stats)
    }
    /// Run `work` with rebuilds held, then rebuild once if any step asked for one:
    /// Enter dispatches a field's pending action and then commits it, and each of
    /// those rebuilds on its own.
    pub(crate) fn coalesce_rebuilds<T>(
        &mut self,
        editor: &mut Editor,
        work: impl FnOnce(&mut Self, &mut Editor) -> ara_ui::Result<T>,
    ) -> ara_ui::Result<T> {
        let held = std::mem::replace(&mut self.hold_rebuilds, true);
        let result = work(self, editor);
        self.hold_rebuilds = held;
        if !held && std::mem::take(&mut self.held_rebuild) {
            self.rebuild(editor)?;
        }
        result
    }
    /// Rebuild unless the last rebuild already reflects the editor's panels and store
    /// revisions (an action that rebuilt in dispatch changed them); true if it rebuilt.
    pub fn rebuild_if_stale(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.built == (editor.panels_revision, editor.host.store().revision()) {
            return Ok(false);
        }
        self.rebuild(editor)?;
        Ok(true)
    }
    fn rebuild_all(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.built = (editor.panels_revision, editor.host.store().revision());
        // The keymap follows two settings; rebuilding it took ~0.1 ms per rebuild.
        let settings = keymap_settings(&editor.state);
        if settings != self.keymap_settings {
            self.keymap = commands::keymap(&editor.state);
            self.keymap_settings = settings;
            self.keymap_revision += 1;
        }
        self.reduce_motion = editor.state.reduce_motion;
        self.version += 1;
        self.bindings.clear();
        self.fields.clear();
        self.browser.poll();
        self.vfx_visible = false;
        self.follow_focus(editor)?;
        self.rebuild_main(editor, true)?;
        // Floating surfaces follow the layout: drop closed ones, build open ones.
        let floating = editor.state.floating.clone();
        self.floats
            .retain(|s| floating.iter().any(|f| f.id == s.id));
        for window in &floating {
            if !self.floats.iter().any(|s| s.id == window.id) {
                let size = [
                    window.rect[2].max(160) as f32,
                    window.rect[3].max(120) as f32,
                ];
                self.floats.push(Surface {
                    id: window.id.clone(),
                    ui: Ui::editor("<ui id='float' />", include_str!("editor.css"), size)?,
                    input: InputState::default(),
                    pointer: [0.; 2],
                });
            }
            self.enter(&window.id);
            let result = self.rebuild_float(editor, window);
            self.leave();
            result?;
        }
        if !self.vfx_visible {
            self.vfx_stream.pause();
        }
        // A closed panel's body is not kept.
        self.bodies
            .retain(|panel, _| crate::state::panel_visible(&editor.state, panel));
        Ok(())
    }
    /// The node with `id` if it is shown: inactive tabs keep their nodes with stale
    /// geometry, so pointer and geometry code finds nodes through this.
    pub(crate) fn shown_node(&self, id: &str) -> Option<usize> {
        self.ui.find(id).filter(|i| self.ui.shown(*i))
    }
    fn stylesheet(editor: &Editor) -> String {
        let mut css = include_str!("editor.css").to_owned();
        if editor.state.comfortable() {
            css.push_str(include_str!("editor-comfortable.css"));
        }
        css
    }
    /// Reconcile markup into the current surface (nodes whose ID survives keep their
    /// shaped text, layout, widget and hover/focus state), lay it out once and mount
    /// the virtual rows. Stylesheet and theme apply only when they change. The caller
    /// frames afterwards: new rows have geometry only after that frame.
    fn mount(&mut self, editor: &mut Editor, markup: &str) -> ara_ui::Result<()> {
        let started = std::time::Instant::now();
        let result = self.mount_markup(editor, markup);
        self.stats.mount_ms += started.elapsed().as_secs_f64() * 1000.;
        result
    }
    fn mount_markup(&mut self, editor: &mut Editor, markup: &str) -> ara_ui::Result<()> {
        self.ui.set_stylesheet(&Self::stylesheet(editor))?;
        self.ui.switch_theme(if editor.state.light {
            ara_ui::theme::EditorTheme::Light
        } else {
            ara_ui::theme::EditorTheme::Dark
        })?;
        let reconciling = std::time::Instant::now();
        // Area headers and tab bodies are parts: an unchanged one keeps its retained
        // subtree, virtual rows included.
        let parts = std::mem::take(&mut self.parts);
        let borrowed: Vec<(&str, &str)> = parts
            .iter()
            .map(|(host, part)| (host.as_str(), part.as_str()))
            .collect();
        self.ui.reconcile_parts(markup, &borrowed)?;
        // Inactive tabs stay mounted but hidden, their markup kept for the next build.
        for (host, part) in parts {
            let Some(panel) = host.strip_prefix("body-") else {
                continue;
            };
            if let Some(i) = self.ui.find(&host) {
                self.ui
                    .set_visible(i, !self.hidden_bodies.contains(&host))?;
            }
            if panels::keep_alive(panel) {
                self.bodies.insert(panel.to_owned(), part);
            }
        }
        self.stats.reconcile_ms += reconciling.elapsed().as_secs_f64() * 1000.;
        let ui = &mut self.ui;
        for hidden in ["tooltip", "drop-label", "gizmo-readout"] {
            if let Some(i) = ui.find(hidden) {
                ui.set_visible(i, false)?;
            }
        }
        self.tooltip_text = None;
        let framing = std::time::Instant::now();
        self.stats.work += ui.frame(&mut self.text)?;
        self.stats.frame_ms += framing.elapsed().as_secs_f64() * 1000.;
        let rows = std::time::Instant::now();
        let row = if editor.state.comfortable() { 26. } else { 22. };
        let surface = self.current.clone();
        // A kept part keeps its row pool: the rows follow their nodes and fit the
        // panel again; a recompiled part mounts a new pool. A hidden tab's pool waits,
        // as it is, for the tab to show again.
        let (mut bind_hierarchy, mut bind_assets) = (false, false);
        if let Some(index) = ui.find("hierarchy-rows") {
            let kept = self.hierarchy_surface == surface
                && self.hierarchy_view.as_mut().is_some_and(|v| v.remap(ui));
            if !ui.shown(index) {
                if !kept && self.hierarchy_surface == surface {
                    self.hierarchy_view = None;
                }
            } else {
                let height = ui.nodes[index].rect.h.max(row);
                let width = ui.nodes[index].rect.w.max(80.);
                let view = match self.hierarchy_view.as_mut() {
                    Some(view) if kept => {
                        view.fit(ui, height, vec![width])?;
                        view
                    }
                    _ => self.hierarchy_view.insert(VirtualView::mount(
                        ui,
                        index,
                        row,
                        height,
                        vec![width],
                    )?),
                };
                view.scroll(ui, &self.hierarchy, self.hierarchy_offset, height)?;
                bind_hierarchy = true;
                self.hierarchy_surface = surface.clone();
            }
        } else if self.hierarchy_surface == surface {
            self.hierarchy_view = None;
        }
        // The list's columns: the name, then its type (or folder, when searching).
        let mut asset_columns = [0.; 2];
        if let Some(index) = ui.find("asset-rows") {
            let kept = self.asset_surface == surface
                && self.asset_view.as_mut().is_some_and(|v| v.remap(ui));
            if !ui.shown(index) {
                if !kept && self.asset_surface == surface {
                    self.asset_view = None;
                }
            } else {
                let height = ui.nodes[index].rect.h.max(row);
                let width = ui.nodes[index].rect.w.max(120.) - 8.;
                let detail = names::detail_width(width);
                asset_columns = [width - detail, detail];
                let view = match self.asset_view.as_mut() {
                    Some(view) if kept => {
                        view.fit(ui, height, asset_columns.to_vec())?;
                        view
                    }
                    _ => self.asset_view.insert(VirtualView::mount(
                        ui,
                        index,
                        row,
                        height,
                        asset_columns.to_vec(),
                    )?),
                };
                view.scroll(ui, &self.asset_rows, self.asset_offset, height)?;
                bind_assets = true;
                self.asset_surface = surface;
            }
        } else if self.asset_surface == surface {
            self.asset_view = None;
        }
        if bind_hierarchy {
            self.bind_hierarchy_rows();
        }
        if bind_assets {
            self.bind_asset_rows();
        }
        // Row decorations set attributes only; the caller's frame lays out and paints
        // once (no geometry is read until then, apart from pre-existing nodes).
        self.decorate_hierarchy();
        if self.asset_surface == self.current
            && let Some(view) = &self.asset_view
        {
            // Name cells leave room for the painted type icon (`polish`); the detail
            // column is muted. The selected asset's row is highlighted.
            let selected = editor
                .state
                .asset_path
                .as_deref()
                .map(ara_ui::source::stable_id);
            let style = |column: usize, selected: bool| match column {
                0 => format!(
                    "height:{row}px;width:{}px;padding:2px;padding-left:30px;overflow:hidden;font-size:12px",
                    asset_columns[0]
                ),
                _ => format!(
                    "height:{row}px;width:{}px;padding:2px;padding-right:8px;overflow:hidden;font-size:11px;color:var(--{})",
                    asset_columns[1],
                    if selected {
                        "selection-muted-foreground"
                    } else {
                        "muted-foreground"
                    }
                ),
            };
            for index in view.visible_nodes() {
                let key = self.ui.nodes[*index]
                    .element
                    .id
                    .rsplit_once("key-")
                    .and_then(|(_, s)| s.parse::<u64>().ok());
                let class = if key.is_some() && key == selected {
                    "virtual-row row-selected"
                } else {
                    "virtual-row"
                };
                let node = &mut self.ui.nodes[*index];
                if node.element.attrs.get("class").map(String::as_str) != Some(class) {
                    node.element.attrs.insert("class".into(), class.into());
                    node.dirty.style = true;
                }
                for n in 0..2 {
                    let Some(&child) = self.ui.nodes[*index].element.children.get(n) else {
                        continue;
                    };
                    let style = style(n, class.contains("row-selected"));
                    let node = &mut self.ui.nodes[child];
                    if node.element.attrs.get("style") != Some(&style) {
                        node.element.attrs.insert("style".into(), style);
                        node.dirty.style = true;
                    }
                }
            }
        }
        self.stats.rows_ms += rows.elapsed().as_secs_f64() * 1000.;
        Ok(())
    }
    fn rebuild_float(
        &mut self,
        editor: &mut Editor,
        window: &ara_ui::docking::Floating,
    ) -> ara_ui::Result<()> {
        self.parts.clear();
        self.hidden_bodies.clear();
        let mut markup = String::from(
            "<ui id='float'><panel id='float-root' class='dock-root' style='padding-top:4px'>",
        );
        self.dock(
            &window.root,
            &format!("f{}", window.id),
            false,
            editor,
            &mut markup,
        )?;
        markup.push_str("</panel>");
        self.overlays(editor, &mut markup)?;
        markup.push_str(POPUP_TAIL);
        markup.push_str("</ui>");
        self.mount(editor, &markup)?;
        self.stats.work += self.ui.frame(&mut self.text)?;
        if self.frame_new_graphs(editor)? {
            self.stats.work += self.ui.frame(&mut self.text)?;
        }
        self.after_mount(editor)
    }
    fn rebuild_main(
        &mut self,
        editor: &mut Editor,
        allow_asset_reflow: bool,
    ) -> ara_ui::Result<()> {
        self.parts.clear();
        self.hidden_bodies.clear();
        let mut markup = String::from("<ui id='editor'>");
        // The menubar and status bar are parts too: their frequent changes (the play
        // state, the focused panel's hint) update in place instead of the skeleton.
        let mut menubar = String::new();
        self.menubar(editor, &mut menubar);
        markup.push_str("<panel id='menubar' />");
        self.parts.push(("menubar".into(), menubar));
        if editor.state.playing.is_some() {
            markup.push_str("<panel id='play-line' class='play-line' />");
        }
        markup.push_str("<panel id='dock-root' class='dock-root'>");
        let layout = editor.state.layout.clone();
        match editor.state.maximized.clone() {
            Some(panel) if crate::state::panel_visible(&editor.state, &panel) => {
                let area = ara_ui::docking::DockNode::Area {
                    id: "maximized".into(),
                    tabs: vec![panel],
                    active: 0,
                };
                self.dock(&area, "max", false, editor, &mut markup)?;
            }
            _ => self.dock(&layout, "root", true, editor, &mut markup)?,
        }
        markup.push_str("</panel>");
        if editor.state.canvas_add
            && !editor.state.palette
            && let Some(doc) = self.resource_for(editor, "material")
        {
            self.node_palette(editor, &mut markup, &doc)?;
        }
        let mut statusbar = String::new();
        self.statusbar(editor, &mut statusbar);
        markup.push_str("<panel id='statusbar' />");
        self.parts.push(("statusbar".into(), statusbar));
        self.overlays(editor, &mut markup)?;
        if editor.state.palette {
            self.palette(editor, &mut markup)?;
        }
        if let Some(dialog) = self.dialog.take() {
            self.dialog_markup(&dialog, &mut markup);
            self.dialog = Some(dialog);
        }
        if self.rename
            && let Some(id) = editor.state.selection.first()
        {
            let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
                .map_err(|d| format!("{d:?}"))?;
            let name = resolved
                .get(id)
                .and_then(|e| e.record.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(id);
            self.bindings.insert(
                ara_ui::source::stable_id("entity-rename-value"),
                Binding::RenameValue,
            );
            markup.push_str(&format!("<field id='entity-rename-value' style='position:absolute;left:0px;top:0px;width:180px;height:22px' text='{}' label='Entity name' action='Dispatch' />",escape(name)));
        }
        markup.push_str(POPUP_TAIL);
        markup.push_str("</ui>");
        self.mount(editor, &markup)?;
        self.menu_anchors = (0..commands::menus::MENUS.len())
            .filter_map(|i| self.ui.find(&format!("menu-title-{i}")))
            .map(|i| self.ui.nodes[i].rect.x)
            .collect();
        if editor.state.focus == "vfx"
            && let Some(item) = editor.state.resource_item.as_deref()
        {
            let parts = item.split('|').collect::<Vec<_>>();
            if let [emitter, stage, name] = parts.as_slice()
                && let (Some(list), Some(module)) = (
                    self.ui.find("vfx-stack-list"),
                    self.ui
                        .find(&format!("module-{emitter}-{stage}-{name}-card")),
                )
            {
                let list_rect = self.ui.nodes[list].rect;
                let module_rect = self.ui.nodes[module].rect;
                if module_rect.y + module_rect.h > list_rect.y + list_rect.h {
                    self.ui.scroll_to(
                        list,
                        [
                            0.,
                            (module_rect.y - list_rect.y - list_rect.h * 0.35).max(0.),
                        ],
                    )?;
                }
            }
        }
        if let (Some(cube), Some(rect)) = (self.ui.find("view-cube"), self.viewport()) {
            let r = self.ui.nodes[cube].rect;
            self.ui.nodes[cube].paint_offset =
                [rect.x + rect.w - axis::SIZE - 12. - r.x, rect.y + 12. - r.y];
            self.ui.nodes[cube].dirty.paint = true;
        }
        self.stats.work += self.ui.frame(&mut self.text)?;
        if self.frame_new_graphs(editor)? {
            self.stats.work += self.ui.frame(&mut self.text)?;
        }
        if allow_asset_reflow
            && self.asset_surface.is_none()
            && self.shown_node("asset-grid").is_some_and(|i| {
                let rect = self.ui.nodes[i].rect;
                (rect.w - self.asset_grid_size[0]).abs() > 0.5
                    || (rect.h - self.asset_grid_size[1]).abs() > 0.5
            })
        {
            // One measured reflow handles first-open, splitter and monitor-size changes.
            return self.rebuild_main(editor, false);
        }
        let canvas_modal = editor.state.canvas_add
            && !editor.state.palette
            && self.ui.find("node-palette").is_some();
        if !canvas_modal && self.canvas_modal_open {
            self.input.pop(&mut self.ui);
        }
        let modal = editor.state.palette || self.dialog.is_some();
        if modal && !self.modal_open {
            self.panel_opened = (!self.reduce_motion).then(std::time::Instant::now);
            let (root, field) = if editor.state.palette {
                ("palette", Some("palette-search"))
            } else {
                ("dialog", self.dialog.as_ref().and_then(Dialog::field))
            };
            self.input.push(
                &mut self.ui,
                ara_ui::source::stable_id(root),
                field.map(ara_ui::source::stable_id),
                true,
            )?;
        }
        if !modal && self.modal_open {
            self.input.pop(&mut self.ui);
        }
        self.modal_open = modal;
        if canvas_modal && !self.canvas_modal_open {
            self.panel_opened = (!self.reduce_motion).then(std::time::Instant::now);
            self.input.push(
                &mut self.ui,
                ara_ui::source::stable_id("node-palette"),
                Some(ara_ui::source::stable_id("search-nodes")),
                true,
            )?;
        }
        self.canvas_modal_open = canvas_modal;
        if editor.state.palette {
            let _ = self
                .input
                .focus(&mut self.ui, ara_ui::source::stable_id("palette-search"));
        } else if let Some(field) = self.dialog.as_ref().and_then(Dialog::field) {
            let _ = self
                .input
                .focus(&mut self.ui, ara_ui::source::stable_id(field));
        } else if self.menu.is_some()
            && let Some(_) = self.ui.find("menu-filter")
        {
            let _ = self
                .input
                .focus(&mut self.ui, ara_ui::source::stable_id("menu-filter"));
        }
        if canvas_modal {
            let _ = self
                .input
                .focus(&mut self.ui, ara_ui::source::stable_id("search-nodes"));
        }
        if self.rename
            && let Some(i) = self.ui.find("entity-rename-value")
        {
            if let Some(row) = editor.state.selection.first().and_then(|e| {
                self.hierarchy_view
                    .as_ref()
                    .filter(|_| self.hierarchy_surface.is_none())
                    .and_then(|v| self.ui.by_id(v.identity(crate::world::render_id(e))))
            }) {
                let r = self.ui.nodes[row].rect;
                self.ui.nodes[i].paint_offset = [r.x + 40., r.y];
                self.ui.nodes[i].dirty.paint = true;
                self.stats.work += self.ui.frame(&mut self.text)?;
            }
            self.input.focus(
                &mut self.ui,
                ara_ui::source::stable_id("entity-rename-value"),
            )?;
        }
        self.after_mount(editor)
    }
    /// Paint-time decorations shared by every surface. They are spliced in under the
    /// popup layer (menus, palette, dialogs, tooltips), which stays on top.
    fn paint_decorations(&mut self) {
        let mut start = self.ui.display.commands.len();
        self.paint_swatches();
        self.paint_polish();
        // Canvases splice in under their own panels, inside the dock tree: indices
        // after those points (`start`, and the popup start measured before painting)
        // move by what went in.
        let splices = self.paint_graphs();
        let shift = |index: usize| {
            splices
                .iter()
                .fold(index, |i, &(at, n)| if at <= i { i + n } else { i })
        };
        start = shift(start);
        self.paint_drop_preview();
        if let Some(mut at) = self.popup_start()
            && at < start
        {
            at = shift(at);
            let decorations: Vec<_> = self.ui.display.commands.drain(start..).collect();
            self.ui.display.commands.splice(at..at, decorations);
        }
        // The axis triad splices its stems in under its own caps.
        self.paint_axis();
    }
    /// Display-list index of the first node painted after the dock tree: the
    /// status bar, then menus, the palette, dialogs and tooltips.
    fn popup_start(&self) -> Option<usize> {
        let ui = &self.ui;
        let root = ui.find("dock-root").or_else(|| ui.find("float-root"))?;
        let siblings = &ui.nodes[ui.nodes[root].element.parent?].element.children;
        let later = &siblings[siblings.iter().position(|c| *c == root)? + 1..];
        let at = ui.paint_order.iter().position(|i| later.contains(i))?;
        ui.paint_starts.get(at).copied()
    }
    /// Run after every mount: decorations, a pending Inspector field reveal, motion.
    fn after_mount(&mut self, editor: &Editor) -> ara_ui::Result<()> {
        self.paint_decorations();
        if self.field_request != editor.state.focus_field
            && self.shown_node("inspector-scroll").is_some()
        {
            if let Some(path) = editor.state.focus_field.as_deref() {
                self.reveal_field(path)?;
            }
            self.field_request = editor.state.focus_field.clone();
        }
        self.motion
            .apply(&mut self.ui.display, true, self.reduce_motion);
        Ok(())
    }
    /// Scrolls the inspector to the field at `path` (`Transform.position.x`) and focuses it.
    pub(crate) fn reveal_field(&mut self, path: &str) -> ara_ui::Result<()> {
        let (path, axis) = path
            .rsplit_once('.')
            .and_then(|(base, axis)| {
                ["x", "y", "z", "w"]
                    .iter()
                    .position(|v| *v == axis)
                    .map(|n| (base, Some(n)))
            })
            .unwrap_or((path, None));
        let id = path.replace('.', "-");
        let Some(row) = self
            .ui
            .find(&format!("field-{id}"))
            .or_else(|| self.ui.find(&format!("component-{id}")))
        else {
            return Ok(());
        };
        if let Some(scroll) = self.ui.find("inspector-scroll") {
            // Rects are on screen (already scrolled); `scroll_to` takes the absolute
            // offset, so the row's distance from the top adds to the current one.
            let delta = self.ui.nodes[row].rect.y - self.ui.nodes[scroll].rect.y;
            let key = ara_ui::source::stable_id(&self.ui.nodes[scroll].element.id);
            let current = match self.ui.widgets.get(&key) {
                Some(ara_ui::widgets::Widget::Scroll(offset)) => offset[1],
                _ => 0.,
            };
            self.ui.scroll_to(scroll, [0., current + delta])?;
            self.stats.work += self.ui.frame(&mut self.text)?;
        }
        if let Some(field) = self
            .ui
            .find(&format!("edit-{id}"))
            .or_else(|| self.ui.find(&format!("edit-{id}-{}", axis.unwrap_or(0))))
        {
            let key = ara_ui::source::stable_id(&self.ui.nodes[field].element.id);
            self.input.focus(&mut self.ui, key)?;
        }
        Ok(())
    }
    /// Complete decorative transitions before deterministic review captures.
    pub fn settle_transitions(&mut self) {
        self.motion = Default::default();
        self.panel_opened = None;
        if let Some(i) = self
            .ui
            .find("palette")
            .or_else(|| self.ui.find("node-palette"))
        {
            self.ui.nodes[i].paint_offset = [0.; 2];
        }
        for n in &mut self.ui.nodes {
            n.dirty.paint = true;
        }
    }
    /// Update the low-frequency status readout without rebuilding panels.
    pub fn frame_timing(&mut self, hz: f64, ms: f64) -> ara_ui::Result<()> {
        if let Some(i) = self.ui.find("status-timing") {
            self.ui.set_text(i, &format!("{hz:.0} FPS · {ms:.2} ms"))?;
        }
        Ok(())
    }
    /// Run retained style/layout/paint work for the current surface; unchanged frames
    /// return zero work counters.
    pub fn frame(&mut self) -> ara_ui::Result<ara_ui::Work> {
        if let Some(start) = self.panel_opened {
            let p = (start.elapsed().as_secs_f32() / 0.14).clamp(0., 1.);
            if let Some(i) = self
                .ui
                .find("palette")
                .or_else(|| self.ui.find("node-palette"))
            {
                self.ui.nodes[i].paint_offset[1] = -4. * (1. - p).powi(3);
                self.ui.nodes[i].dirty.paint = true;
            }
            if p >= 1. || self.reduce_motion {
                self.panel_opened = None;
            }
        }
        let mut work = self.ui.frame(&mut self.text)?;
        self.stats.work += work;
        if work.paint_nodes > 0 {
            self.paint_decorations();
        }
        if self.motion.apply(
            &mut self.ui.display,
            work.paint_nodes > 0,
            self.reduce_motion,
        ) {
            work.paint_nodes = work.paint_nodes.max(1);
        }
        Ok(work)
    }
    /// Frame a floating surface in place; its display list stays on the surface.
    pub fn frame_surface(&mut self, id: &str) -> ara_ui::Result<ara_ui::Work> {
        let returning = self.current.clone();
        if !self.enter(id) {
            return Ok(ara_ui::Work::default());
        }
        let result = self.frame();
        self.leave();
        if let Some(back) = returning {
            self.enter(&back);
        }
        result
    }
    /// Text copied by a command, for the window owner to put on the OS clipboard.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.pending_clipboard.take()
    }
    pub(crate) fn poll_assets(&mut self) -> bool {
        let browser_changed = self.browser.poll();
        if browser_changed {
            // Any project file changed (every autosave does): every tile asks again,
            // since a thumbnail depends on its source's dependencies too. Unchanged ones
            // come straight back from the content cache; until then each tile keeps its
            // image instead of flashing back to its icon.
            self.thumbnails.invalidate_all();
        }
        let mut changed = browser_changed || std::mem::take(&mut self.asset_grid_dirty);
        // An import published (or failed): tiles that were waiting on one ask again now,
        // not on a timer.
        if self.imports.poll() {
            changed = true;
            self.thumbnails.retry_pending();
        }
        let completed = self.thumbnails.poll();
        for result in completed {
            match result {
                crate::asset_tools::thumbnails::Done::Image(thumb) => {
                    self.thumbnail_errors.remove(&thumb.key);
                    let slot = if let Some(slot) = self.thumbnail_slots.get(&thumb.key) {
                        *slot
                    } else {
                        let slot = self.thumbnail_next % 48;
                        self.thumbnail_next += 1;
                        if let Some(key) = self
                            .thumbnail_slots
                            .iter()
                            .find_map(|(key, v)| (*v == slot).then(|| key.clone()))
                        {
                            self.thumbnails.invalidate(&key);
                            self.thumbnail_slots.remove(&key);
                            self.thumbnail_revision += 1;
                        }
                        self.thumbnail_slots.insert(thumb.key, slot);
                        self.thumbnail_revision += 1;
                        slot
                    };
                    let name = crate::asset_tools::thumbnails::image_name(slot);
                    self.thumbnail_uploads.push((
                        ara_ui::source::stable_id(&name) as u32 & 0x7fff_ffff,
                        ara_ui_render::gpu::Image {
                            width: thumb.image.width(),
                            height: thumb.image.height(),
                            rgba: thumb.image.into_raw(),
                        },
                    ));
                    changed = true;
                }
                // Waiting on its import: the tile shows the import's pending state.
                crate::asset_tools::thumbnails::Done::Pending(_) => {}
                crate::asset_tools::thumbnails::Done::Failed(key, path, error) => {
                    // A node preview that cannot render: its node keeps a compact body.
                    if key.starts_with(crate::asset_tools::thumbnails::NODE) {
                        self.node_previews.failed.insert(key);
                    } else {
                        self.pending_reports
                            .push(("thumbnail", format!("{path}: no preview ({error})")));
                        self.thumbnail_errors.insert(key, error);
                    }
                    changed = true;
                }
            }
        }
        let mut live = false;
        for result in self.vfx_stream.poll() {
            match result {
                Ok(frame) => {
                    changed |= !self.vfx_ready || self.vfx_error.is_some();
                    self.vfx_ready = true;
                    self.vfx_error = None;
                    self.thumbnail_uploads.push((
                        ara_ui::source::stable_id("vfx-live-preview") as u32 & 0x7fff_ffff,
                        ara_ui_render::gpu::Image {
                            width: frame.image.width(),
                            height: frame.image.height(),
                            rgba: frame.image.into_raw(),
                        },
                    ));
                    self.vfx_live = Some(vfx_panel::VfxLive {
                        tick: frame.tick,
                        alive: frame.alive,
                        emitters: frame.emitters,
                    });
                    live = true;
                }
                Err(error) => {
                    self.vfx_error = Some(error);
                    changed = true;
                }
            }
        }
        // Counters change every preview frame: update their text, not the panel.
        if live && !changed {
            let _ = self.vfx_live_text();
        }
        changed
    }
    /// Drain completed background thumbnails into the same-device UI image registry.
    pub fn upload_thumbnails(
        &mut self,
        renderer: &mut ara_ui_render::gpu::Renderer,
    ) -> ara_ui::Result<bool> {
        let changed = !self.thumbnail_uploads.is_empty();
        for (id, image) in self.thumbnail_uploads.drain(..) {
            renderer.image(id, image)?;
        }
        Ok(changed)
    }
    /// Current scene image rectangle in the main window; also the picking boundary.
    pub fn viewport(&self) -> Option<Box2> {
        if self.current.is_some() {
            return None;
        }
        self.ui
            .find("viewport-image")
            .map(|i| self.ui.nodes[i].rect)
    }
    /// Compact agent layout report; glyph shaping internals stay out of capture jobs.
    pub fn layout_report(&self) -> Value {
        json!({"size":self.ui.size,"focus":self.input.focused(),"graph":self.ui.find(&graph_panel::canvas_id(self.graph_page)).and_then(|i|self.active_graph().map(|g|g.canvas.report(self.ui.nodes[i].rect))),"nodes":self.ui.paint_order.iter().map(|i|{let n=&self.ui.nodes[*i];json!({"id":n.element.id,"stableId":ara_ui::source::stable_id(&n.element.id),"tag":n.element.tag,"box":n.rect,"clip":n.clip,"text":n.text.as_ref().map(|t|t.value.as_str()),"disabled":n.state.disabled})}).collect::<Vec<_>>()})
    }
    /// Visible asset payload centre for scripted drags through the production drop route.
    pub fn asset_point(&self, path: &str) -> Option<[f32; 2]> {
        self.ui.paint_order.iter().find_map(|i| {
            let node = &self.ui.nodes[*i];
            match self
                .bindings
                .get(&ara_ui::source::stable_id(&node.element.id))
            {
                Some(Binding::Asset(asset)) if asset.path == path => Some([
                    node.rect.x + node.rect.w * 0.5,
                    node.rect.y + node.rect.h * 0.5,
                ]),
                _ => None,
            }
        })
    }
    pub(crate) fn preview_ready(&self, panel: &str) -> bool {
        if panel == "vfx" {
            self.vfx_ready
        } else if panel == "material" {
            self.material_preview_key
                .as_ref()
                .is_some_and(|key| self.thumbnail_slots.contains_key(key))
        } else {
            true
        }
    }
    pub(crate) fn asset_previews_ready(&self, paths: &[&str]) -> bool {
        paths
            .iter()
            .all(|path| self.thumbnail_slots.contains_key(*path))
    }
    /// Scripted pointer activation through the actual UI input helper.
    pub fn click(
        &mut self,
        id: &str,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<()> {
        let index = self.ui.find(id).ok_or("UI control not found")?;
        let r = self.ui.nodes[index].rect;
        let p = [r.x + r.w * 0.5, r.y + r.h * 0.5];
        self.pointer = p;
        self.input.event(
            &mut self.ui,
            ara_ui::input::InputEvent::PointerDown(p),
            clipboard,
        )?;
        self.input.event(
            &mut self.ui,
            ara_ui::input::InputEvent::PointerUp(p),
            clipboard,
        )
    }
    /// Scripted hierarchy selection through its visible virtual row control.
    pub fn click_entity(
        &mut self,
        entity: &str,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<()> {
        let view = self
            .hierarchy_view
            .as_ref()
            .filter(|_| self.hierarchy_surface == self.current)
            .ok_or("hierarchy panel is not visible")?;
        let id = view.identity(crate::world::render_id(entity));
        let index = self
            .ui
            .by_id(id)
            .ok_or("entity is outside the hierarchy viewport")?;
        let r = self.ui.nodes[index].rect;
        let p = [r.x + 64., r.y + r.h * 0.5];
        self.pointer = p;
        self.input.event(
            &mut self.ui,
            ara_ui::input::InputEvent::PointerDown(p),
            clipboard,
        )?;
        self.input.event(
            &mut self.ui,
            ara_ui::input::InputEvent::PointerUp(p),
            clipboard,
        )
    }
    pub(crate) fn bind_button(
        &mut self,
        id: &str,
        text: &str,
        binding: Binding,
        class: &str,
    ) -> String {
        self.bindings.insert(ara_ui::source::stable_id(id), binding);
        button(id, text, class)
    }
    /// Bind a toolbar or chrome control to a registry command.
    pub(crate) fn bind_command(
        &mut self,
        id: &str,
        command: &'static str,
        target: commands::Target,
    ) {
        self.bindings.insert(
            ara_ui::source::stable_id(id),
            Binding::Command(command, target),
        );
    }
    /// Every chrome/toolbar/menu control's registry command, for UI tests.
    pub fn command_bindings(&self) -> Vec<(String, &'static str)> {
        self.ui
            .nodes
            .iter()
            .filter_map(
                |n| match self.bindings.get(&ara_ui::source::stable_id(&n.element.id)) {
                    Some(Binding::Command(c, _)) => Some((n.element.id.clone(), *c)),
                    _ => None,
                },
            )
            .collect()
    }
    /// Buttons in the current surface whose binding is not a registry command.
    pub fn uncommanded_buttons(&self, prefixes: &[&str]) -> Vec<String> {
        self.ui
            .nodes
            .iter()
            .filter(|n| {
                n.element.tag == "button" && prefixes.iter().any(|p| n.element.id.starts_with(p))
            })
            .filter(|n| {
                !matches!(
                    self.bindings.get(&ara_ui::source::stable_id(&n.element.id)),
                    Some(
                        Binding::Command(..)
                            | Binding::MenuBar(_)
                            | Binding::MenuSub(..)
                            | Binding::ContextMenu(..)
                    )
                )
            })
            .map(|n| n.element.id.clone())
            .collect()
    }
}
/// Hidden nodes every surface keeps for in-place updates: tooltip and drop label.
const POPUP_TAIL: &str = "<tooltip id='tooltip' class='tooltip'><text id='tooltip-text' class='tooltip-copy' text='' /></tooltip><panel id='drop-label' class='drop-label' style='left:0px;top:0px;width:180px'><text id='drop-label-text' text='' /></panel>";
impl Shell {
    /// Show (or hide) the gizmo readout pill next to the pointer, without a rebuild.
    pub fn show_readout(&mut self, readout: Option<(String, [f32; 2])>) {
        let (Some(pill), Some(text), Some(rect)) = (
            self.ui.find("gizmo-readout"),
            self.ui.find("gizmo-readout-text"),
            self.viewport(),
        ) else {
            return;
        };
        match readout {
            Some((value, at)) => {
                let _ = self.ui.set_text(text, &value);
                let _ = self.ui.set_visible(pill, true);
                let node = &mut self.ui.nodes[pill];
                let base = [
                    node.rect.x - node.paint_offset[0],
                    node.rect.y - node.paint_offset[1],
                ];
                let x = (at[0] + 18.).min(rect.x + rect.w - 220.);
                let y = (at[1] + 18.).min(rect.y + rect.h - 30.);
                node.paint_offset = [x - base[0], y - base[1]];
                node.dirty.paint = true;
            }
            None => {
                let _ = self.ui.set_visible(pill, false);
            }
        }
    }
}
/// The project's name as people read it: `name` in `ara.project.json`, else its
/// folder's name. The title bar and the loading window show it.
pub(crate) fn project_name(editor: &Editor) -> String {
    editor
        .state
        .settings
        .get("project.name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let folder = editor.host.root().file_name()?;
            Some(folder.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Project".into())
}
impl Shell {
    /// Set a floating surface's logical size after its OS window opened or resized
    /// (`Ui::resize`: the next frame lays the surface out at that size).
    pub fn resize_surface(&mut self, id: &str, size: [f32; 2]) -> ara_ui::Result<()> {
        if let Some(surface) = self.floats.iter_mut().find(|s| s.id == id) {
            surface.ui.resize(size)?;
        } else if self.current.as_deref() == Some(id) {
            self.ui.resize(size)?;
        }
        Ok(())
    }
    /// A floating surface's display list and size, for its window's renderer.
    pub fn surface(&self, id: &str) -> Option<&Surface> {
        self.floats.iter().find(|s| s.id == id)
    }
}
/// The settings a keymap is built from (`commands::keymap`).
fn keymap_settings(state: &crate::state::EditorState) -> (Option<Value>, Option<Value>) {
    (
        state.settings.get("input.keymap").cloned(),
        state.settings.get("input.shortcuts").cloned(),
    )
}
