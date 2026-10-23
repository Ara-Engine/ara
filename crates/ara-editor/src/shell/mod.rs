//! Retained native panel composition and semantic interaction helpers.
mod assets;
mod axis;
mod drag;
pub(crate) mod icons;
mod input;
mod keyboard;
mod motion;
mod polish;
mod scrub;
use drag::Drag;
mod animation_panel;
mod assets_panel;
mod graph_input;
mod graph_panel;
mod hierarchy;
mod import_panel;
mod inspector_panel;
mod material_inspector;
pub mod overlay;
mod palette;
mod panels;
mod profiler_panel;
mod review_panel;
mod vfx_panel;
use crate::{Editor, inspector::Field};
use ara_ui::{
    Ui,
    input::InputState,
    paint::Box2,
    virtualize::{VirtualList, VirtualView},
};
use ara_ui_text::TextSystem;
use panels::{button, escape};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Native shell and its retained widget state; no authoring document is mirrored here.
pub struct Shell {
    /// Native runtime tree and renderer-neutral display list.
    pub ui: Ui,
    /// Explicit bundled fonts and retained shaping contexts.
    pub text: TextSystem,
    /// Native focus, capture and keyboard navigation.
    pub input: InputState,
    /// Active schema field models keyed by widget identity.
    pub fields: BTreeMap<String, Field>,
    /// Pointer location in logical pixels.
    pub pointer: [f32; 2],
    /// Shift/control/alt modifiers, updated from physical keys.
    pub modifiers: [bool; 3],
    /// Current tree scroll offset, independent of the finite widget pool.
    pub hierarchy_offset: f32,
    pub(crate) bindings: BTreeMap<u64, Binding>,
    hierarchy: VirtualList,
    hierarchy_entities: BTreeMap<u64, String>,
    hierarchy_tints: BTreeMap<u64, &'static str>,
    hierarchy_ghosts: std::collections::BTreeSet<u64>,
    hierarchy_view: Option<VirtualView>,
    browser: assets::Browser,
    asset_rows: VirtualList,
    asset_entries: BTreeMap<u64, assets::Asset>,
    asset_view: Option<VirtualView>,
    asset_offset: f32,
    thumbnails: crate::asset_tools::thumbnails::Thumbnails,
    thumbnail_slots: BTreeMap<String, usize>,
    thumbnail_next: usize,
    thumbnail_errors: BTreeMap<String, String>,
    material_previews: BTreeMap<String, String>,
    thumbnail_uploads: Vec<(u32, ara_ui_render::gpu::Image)>,
    asset_grid_extent: f32,
    asset_grid_dirty: bool,
    drafts: BTreeMap<String, String>,
    pub(crate) context: bool,
    pub(crate) palette_command: Option<String>,
    pub(crate) palette_params: BTreeMap<String, Value>,
    pub(crate) pick_field: Option<String>,
    pub(crate) rename: bool,
    pub(crate) drag: Option<Drag>,
    scrub: Option<scrub::Scrub>,
    motion: motion::Motion,
    reduce_motion: bool,
    panel_opened: Option<std::time::Instant>,
    color_picker: Option<String>,
    modal_open: bool,
    collapsed_components: std::collections::BTreeSet<String>,
    component_menu: Option<String>,
    hierarchy_style: BTreeMap<u64, polish::RowStyle>,
    selected_rows: std::collections::BTreeSet<u64>,
    pub(crate) version: u64,
    tooltip_text: Option<String>,
    field_request: Option<String>,
    canvas: Option<crate::graph::Canvas>,
    canvas_display: ara_ui::paint::DisplayList,
    canvas_doc: Option<String>,
    canvas_revision: u64,
    canvas_navigation: u64,
    canvas_dragging: bool,
    resource_text: Option<(String, String)>,
    vfx_stream: crate::asset_tools::vfx_stream::VfxStream,
    vfx_visible: bool,
    vfx_ready: bool,
    vfx_error: Option<String>,
    axis_camera: crate::state::Camera,
}
pub(crate) enum Binding {
    Protocol(&'static str, Value),
    Content(Vec<Value>, String),
    Field(String, Option<usize>),
    Search(String),
    Palette(String),
    PaletteParam(String),
    PaletteRun,
    Select(String),
    Asset(assets::Asset),
    ResourceContent(String, Vec<Value>, String),
    ImportField(String, String),
    ResourceCreate(String),
    ResourceText(String, String),
    ResourceApply,
    PreviewShape(bool),
    GraphLayout(String),
    VfxPlayback(bool),
    AnimationEdit(String),
    AnimationField(String, String, String),
    VfxField(String, String, String),
    MaterialValue(String, String),
    Fix(ara_diag::Diagnostic, usize),
    Approve(String),
    Reject(String),
    PickField(String),
    Rename,
    RenameValue,
    AddComponent,
    Context,
    ColorPicker(String),
    ComponentCollapse(String),
    ComponentMenu(String),
    Scrub(String, usize),
    Diagnostic(ara_diag::Diagnostic),
}
impl Shell {
    /// Build the native shell with the project's initial layout and model snapshots.
    pub fn new(editor: &mut Editor, size: [f32; 2]) -> ara_ui::Result<Self> {
        let text = TextSystem::new(
            [
                include_bytes!("../../assets/fonts/InterVariable.ttf").to_vec(),
                include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf").to_vec(),
                include_bytes!("../../../ara-ui-text/fonts/NotoNaskhArabic.ttf").to_vec(),
            ],
            "'Inter Variable', 'Noto Naskh Arabic'",
        )?;
        let ui = Ui::editor("<ui id='editor' />", include_str!("editor.css"), size)?;
        let mut shell = Self {
            ui,
            text,
            input: InputState::default(),
            fields: BTreeMap::new(),
            pointer: [0.; 2],
            modifiers: [false; 3],
            hierarchy_offset: 0.,
            bindings: BTreeMap::new(),
            hierarchy: VirtualList { items: Vec::new() },
            hierarchy_entities: BTreeMap::new(),
            hierarchy_tints: BTreeMap::new(),
            hierarchy_ghosts: std::collections::BTreeSet::new(),
            hierarchy_view: None,
            browser: assets::Browser::new(editor.host.root()),
            asset_rows: VirtualList { items: Vec::new() },
            asset_entries: BTreeMap::new(),
            asset_view: None,
            asset_offset: 0.,
            thumbnails: crate::asset_tools::thumbnails::Thumbnails::new(editor.host.root()),
            thumbnail_slots: BTreeMap::new(),
            thumbnail_next: 0,
            thumbnail_errors: BTreeMap::new(),
            material_previews: BTreeMap::new(),
            thumbnail_uploads: Vec::new(),
            asset_grid_extent: 0.,
            asset_grid_dirty: false,
            drafts: BTreeMap::new(),
            context: false,
            palette_command: None,
            palette_params: BTreeMap::new(),
            pick_field: None,
            rename: false,
            drag: None,
            scrub: None,
            motion: Default::default(),
            reduce_motion: false,
            panel_opened: None,
            color_picker: None,
            modal_open: false,
            collapsed_components: Default::default(),
            component_menu: None,
            hierarchy_style: BTreeMap::new(),
            selected_rows: Default::default(),
            version: 0,
            tooltip_text: None,
            field_request: None,
            canvas_display: Default::default(),
            canvas: None,
            canvas_doc: None,
            canvas_revision: 0,
            canvas_navigation: 0,
            canvas_dragging: false,
            resource_text: None,
            vfx_stream: crate::asset_tools::vfx_stream::VfxStream::new(editor.host.root()),
            vfx_visible: false,
            vfx_ready: false,
            vfx_error: None,
            axis_camera: Default::default(),
        };
        shell.rebuild(editor)?;
        Ok(shell)
    }
    /// Reconcile a document/navigation notification. Unchanged idle frames never rebuild panels.
    pub fn rebuild(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let focus = self.input.focused();
        self.reduce_motion = editor.state.reduce_motion;
        self.version += 1;
        self.bindings.clear();
        self.fields.clear();
        self.hierarchy_view = None;
        self.asset_view = None;
        self.browser.poll();
        self.vfx_visible = false;
        let mut markup = String::from("<ui id='editor'>");
        markup.push_str("<panel id='topbar' class='topbar'>");
        markup.push_str(&format!("<text id='brand' class='brand' text='ara' /><text id='project-name' text='{}' /><text id='project-divider' class='muted' text='/' /><text id='scene-name' class='muted' text='{}' /><panel id='top-spacer' style='flex-grow:1' />",escape(&project_name(editor)),escape(editor.host.store().workspace().path_of(&editor.state.doc).unwrap_or("Scene"))));
        markup.push_str(&format!("<panel id='play-controls' class='play-pill' style='position:absolute;left:{}px;top:8px;width:100px'>",(self.ui.size[0]-100.)*0.5));
        for (id, name) in [
            ("play", "play"),
            ("pause", "pause"),
            ("step", "step-forward"),
        ] {
            markup.push_str(&format!("<button id='{id}' class='segment-button quiet' label='{id}' disabled='true' description='Play requires ara-sim. A disposable player will be connected here.'>{}</button>",icons::icon(id,name)));
        }
        markup.push_str("</panel><panel id='top-spacer2' style='flex-grow:1' />");

        markup.push_str(&self.bind_button(
            "palette-open",
            "Commands    Ctrl K",
            Binding::Protocol("editor.palette", json!({"open":true})),
            "quiet",
        ));
        markup.push_str(&self.bind_button(
            "theme",
            if editor.state.light { "Dark" } else { "Light" },
            Binding::Protocol("editor.theme", json!({"light":!editor.state.light})),
            "quiet",
        ));
        markup.push_str(&self.bind_button(
            "motion",
            if editor.state.reduce_motion {
                "Motion off"
            } else {
                "Motion on"
            },
            Binding::Protocol(
                "editor.preferences",
                json!({"reduceMotion":!editor.state.reduce_motion}),
            ),
            "quiet small",
        ));
        markup.push_str(
            "</panel><panel id='dock-root' style='flex-grow:1; flex-basis:0px; min-height:0px'>",
        );
        self.dock(&editor.state.layout.clone(), "root", editor, &mut markup)?;
        markup.push_str("</panel>");
        if !self.vfx_visible {
            self.vfx_stream.pause();
        }
        let jobs = editor.action("jobs.list", json!({})).unwrap_or(Value::Null);
        let running = jobs["jobs"]
            .as_array()
            .map(|j| j.iter().filter(|j| j["status"] == "running").count())
            .unwrap_or(0);
        let job_text = jobs["jobs"]
            .as_array()
            .and_then(|j| j.iter().find(|j| j["status"] == "running"))
            .map(|j| {
                format!(
                    "{} {:0.0}%  {}",
                    j["name"].as_str().unwrap_or("Job"),
                    j["progress"].as_f64().unwrap_or(0.) * 100.,
                    j["message"].as_str().unwrap_or("")
                )
            })
            .unwrap_or_else(|| "All jobs complete".into());
        markup.push_str(&format!("<panel id='statusbar' class='statusbar'><panel id='connection-dot' style='width:6px; height:6px; border-radius:3px; background:var(--success-foreground)' /><text id='status' class='muted' text='Connected' />{}<text id='status-docs' class='muted' text='{} entities' /><panel id='status-fill' style='flex-grow:1' />{}<text id='status-jobs' class='muted' text='{}' />{}<text id='status-timing' class='muted' text='144 Hz / 6.94 ms' /></panel>",icons::icon("status-entities","box"),self.hierarchy.items.len(),icons::icon("status-jobs",if running>0 {"activity"} else {"circle-check"}),escape(&job_text),icons::icon("status-frame","gauge")));

        let _ = running;
        if self.context {
            self.context_menu(editor, &mut markup);
        }
        if editor.state.palette {
            self.palette(editor, &mut markup)?;
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
            markup.push_str(&format!("<field id='entity-rename-value' style='position:absolute;left:0px;top:0px;width:180px;height:26px' text='{}' label='Entity name' action='Dispatch' />",escape(name)));
        }
        markup.push_str("<tooltip id='tooltip' class='tooltip'><text id='tooltip-text' class='tooltip-copy' text='' /></tooltip>");
        markup.push_str("</ui>");
        let mut ui = Ui::editor(&markup, include_str!("editor.css"), self.ui.size)?;
        if let Some(i) = ui.find("tooltip") {
            ui.set_visible(i, false)?;
        }
        self.tooltip_text = None;
        if editor.state.light {
            ui.switch_theme(ara_ui::theme::EditorTheme::Light)?;
        }
        ui.frame(&mut self.text)?;
        if let Some(index) = ui.find("hierarchy-rows") {
            let height = ui.nodes[index].rect.h.max(26.);
            let width = ui.nodes[index].rect.w.max(80.);
            let mut view = VirtualView::mount(&mut ui, index, 26., height, vec![width])?;
            view.scroll(&mut ui, &self.hierarchy, self.hierarchy_offset, height)?;
            for (id, entity) in &self.hierarchy_entities {
                self.bindings.insert(
                    view.identity(*id),
                    if self.hierarchy_ghosts.contains(id) {
                        Binding::Protocol("editor.focus", json!({"panel":"agents"}))
                    } else {
                        Binding::Select(entity.clone())
                    },
                );
            }
            self.hierarchy_view = Some(view);
        }
        if let Some(index) = ui.find("asset-rows") {
            let height = ui.nodes[index].rect.h.max(26.);
            let width = ui.nodes[index].rect.w.max(80.);
            let mut view = VirtualView::mount(&mut ui, index, 26., height, vec![width])?;
            view.scroll(&mut ui, &self.asset_rows, self.asset_offset, height)?;
            for (key, asset) in &self.asset_entries {
                self.bindings
                    .insert(view.identity(*key), Binding::Asset(asset.clone()));
            }
            self.asset_view = Some(view);
        }
        ui.frame(&mut self.text)?;
        self.ui = ui;
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
        self.decorate_hierarchy();
        if let (Some(cube), Some(rect)) = (self.ui.find("view-cube"), self.viewport()) {
            let r = self.ui.nodes[cube].rect;
            self.ui.nodes[cube].paint_offset = [rect.x + rect.w - 108. - r.x, rect.y + 12. - r.y];
            self.ui.nodes[cube].dirty.paint = true;
        }
        if let Some(view) = &self.asset_view {
            for index in view.visible_nodes() {
                for n in 0..self.ui.nodes[*index].element.children.len() {
                    let child = self.ui.nodes[*index].element.children[n];
                    let node = &mut self.ui.nodes[child];
                    node.element.attrs.insert("style".into(),"height:26px;width:100%;padding:4px;padding-left:32px;overflow:hidden;font-size:12px".into());
                    node.dirty.style = true;
                }
            }
        }

        self.ui.frame(&mut self.text)?;
        if editor.state.palette && !self.modal_open {
            self.panel_opened = (!self.reduce_motion).then(std::time::Instant::now);
            self.input.push(
                &mut self.ui,
                ara_ui::source::stable_id("palette"),
                Some(ara_ui::source::stable_id("palette-search")),
                true,
            )?;
        }
        if !editor.state.palette && self.modal_open {
            self.input.pop(&mut self.ui);
        }
        self.modal_open = editor.state.palette;
        if let Some(id) = focus {
            let _ = self.input.focus(&mut self.ui, id);
        }
        if editor.state.palette {
            let id = ara_ui::source::stable_id("palette-search");
            let _ = self.input.focus(&mut self.ui, id);
        }
        if self.rename
            && let Some(i) = self.ui.find("entity-rename-value")
        {
            if let Some(row) = editor.state.selection.first().and_then(|e| {
                self.hierarchy_view
                    .as_ref()
                    .and_then(|v| self.ui.by_id(v.identity(crate::world::render_id(e))))
            }) {
                let r = self.ui.nodes[row].rect;
                self.ui.nodes[i].paint_offset = [r.x + 40., r.y];
                self.ui.nodes[i].dirty.paint = true;
                self.ui.frame(&mut self.text)?;
            }
            self.input.focus(
                &mut self.ui,
                ara_ui::source::stable_id("entity-rename-value"),
            )?;
        }
        self.paint_swatches();
        self.paint_polish();
        self.paint_axis();
        self.paint_graph();
        if self.field_request != editor.state.focus_field {
            if let Some(path) = editor.state.focus_field.as_deref() {
                self.reveal_field(path)?;
            }
            self.field_request = editor.state.focus_field.clone();
        }
        self.motion
            .apply(&mut self.ui.display, true, self.reduce_motion);
        Ok(())
    }
    fn reveal_field(&mut self, path: &str) -> ara_ui::Result<()> {
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
        let Some(row) = self.ui.find(&format!("field-{id}")) else {
            return Ok(());
        };
        if let Some(scroll) = self.ui.find("inspector-scroll") {
            let delta = self.ui.nodes[row].rect.y - self.ui.nodes[scroll].rect.y;
            self.ui.scroll_to(scroll, [0., delta])?;
            self.ui.frame(&mut self.text)?;
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
        if let Some(i) = self.ui.find("palette") {
            self.ui.nodes[i].paint_offset = [0.; 2];
        }
        for n in &mut self.ui.nodes {
            n.dirty.paint = true;
        }
    }
    /// Update the low-frequency status readout without rebuilding panels.
    pub fn frame_timing(&mut self, hz: f64, ms: f64) -> ara_ui::Result<()> {
        if let Some(i) = self.ui.find("status-timing") {
            self.ui.set_text(i, &format!("{hz:.0} FPS / {ms:.2} ms"))?;
        }
        Ok(())
    }
    /// Run retained style/layout/paint work; unchanged frames return zero work counters.
    pub fn frame(&mut self) -> ara_ui::Result<ara_ui::Work> {
        if let Some(start) = self.panel_opened {
            let p = (start.elapsed().as_secs_f32() / 0.14).clamp(0., 1.);
            if let Some(i) = self.ui.find("palette") {
                self.ui.nodes[i].paint_offset[1] = -4. * (1. - p).powi(3);
                self.ui.nodes[i].dirty.paint = true;
            }
            if p >= 1. || self.reduce_motion {
                self.panel_opened = None;
            }
        }
        let mut work = self.ui.frame(&mut self.text)?;
        if work.paint_nodes > 0 {
            self.paint_swatches();
            self.paint_polish();
            self.paint_axis();
            self.paint_graph();
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
    pub(crate) fn poll_assets(&mut self) -> bool {
        let browser_changed = self.browser.poll();
        if browser_changed {
            self.thumbnails.invalidate_all();
            self.thumbnail_slots
                .retain(|key, _| key.starts_with("live:"));
        }
        let mut changed = browser_changed || std::mem::take(&mut self.asset_grid_dirty);
        let completed = self.thumbnails.poll().collect::<Vec<_>>();
        for result in completed {
            match result {
                Ok(thumb) => {
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
                        }
                        self.thumbnail_slots.insert(thumb.key, slot);
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
                Err((key, error)) => {
                    eprintln!("thumbnail: {error}");
                    self.thumbnail_errors.insert(key, error);
                    changed = true;
                }
            }
        }
        for result in self.vfx_stream.poll() {
            match result {
                Ok(image) => {
                    changed |= !self.vfx_ready;
                    self.vfx_ready = true;
                    self.vfx_error = None;
                    self.thumbnail_uploads.push((
                        ara_ui::source::stable_id("vfx-live-preview") as u32 & 0x7fff_ffff,
                        ara_ui_render::gpu::Image {
                            width: image.width(),
                            height: image.height(),
                            rgba: image.into_raw(),
                        },
                    ));
                }
                Err(error) => {
                    self.vfx_error = Some(error);
                    changed = true;
                }
            }
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
    /// Current scene image rectangle, also the picking and spatial-input boundary.
    pub fn viewport(&self) -> Option<Box2> {
        self.ui
            .find("viewport-image")
            .map(|i| self.ui.nodes[i].rect)
    }
    /// Compact agent layout report; glyph shaping internals stay out of capture jobs.
    pub fn layout_report(&self) -> Value {
        json!({"size":self.ui.size,"focus":self.input.focused(),"graph":self.ui.find("graph-canvas").and_then(|i|self.canvas.as_ref().map(|c|c.report(self.ui.nodes[i].rect))),"nodes":self.ui.paint_order.iter().map(|i|{let n=&self.ui.nodes[*i];json!({"id":n.element.id,"stableId":ara_ui::source::stable_id(&n.element.id),"tag":n.element.tag,"box":n.rect,"clip":n.clip,"text":n.text.as_ref().map(|t|t.value.as_str()),"disabled":n.state.disabled})}).collect::<Vec<_>>()})
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
            self.ui.find("material-live-preview-thumbnail").is_some()
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
            .ok_or("hierarchy panel is not visible")?;
        let id = view.identity(crate::world::render_id(entity));
        let index = self
            .ui
            .by_id(id)
            .ok_or("entity is outside the hierarchy viewport")?;
        let r = self.ui.nodes[index].rect;
        let p = [r.x + 64., r.y + 13.];
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
}
fn project_name(editor: &Editor) -> String {
    std::fs::read_to_string(editor.host.root().join("ara.project.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["name"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "Project".into())
}
