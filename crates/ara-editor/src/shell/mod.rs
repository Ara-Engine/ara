//! Retained native panel composition and semantic interaction helpers.
mod assets;
mod drag;
mod input;
mod keyboard;
use drag::Drag;
mod assets_panel;
mod hierarchy;
mod inspector_panel;
pub mod overlay;
mod palette;
mod panels;
mod review_panel;
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
    drafts: BTreeMap<String, String>,
    pub(crate) context: bool,
    pub(crate) palette_command: Option<String>,
    pub(crate) palette_params: BTreeMap<String, Value>,
    pub(crate) pick_field: Option<String>,
    pub(crate) rename: bool,
    pub(crate) drag: Option<Drag>,
    color_picker: Option<String>,
    modal_open: bool,
    pub(crate) version: u64,
    tooltip_text: Option<String>,
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
    Fix(ara_diag::Diagnostic, usize),
    Approve(String),
    Reject(String),
    PickField(String),
    Rename,
    RenameValue,
    AddComponent,
    Context,
    ColorPicker(String),
    Diagnostic(ara_diag::Diagnostic),
}
impl Shell {
    /// Build the native shell with the project's initial layout and model snapshots.
    pub fn new(editor: &mut Editor, size: [f32; 2]) -> ara_ui::Result<Self> {
        let text = TextSystem::bundled()?;
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
            drafts: BTreeMap::new(),
            context: false,
            palette_command: None,
            palette_params: BTreeMap::new(),
            pick_field: None,
            rename: false,
            drag: None,
            color_picker: None,
            modal_open: false,
            version: 0,
            tooltip_text: None,
        };
        shell.rebuild(editor)?;
        Ok(shell)
    }
    /// Reconcile a document/navigation notification. Unchanged idle frames never rebuild panels.
    pub fn rebuild(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let focus = self.input.focused();
        self.version += 1;
        self.bindings.clear();
        self.fields.clear();
        self.hierarchy_view = None;
        self.asset_view = None;
        self.browser.poll();
        let mut markup = String::from("<ui id='editor'>");
        markup.push_str("<panel id='topbar' class='topbar'>");
        markup.push_str(&format!("<text id='brand' class='brand' text='ara' /><text id='project-name' text='{}' /><text id='project-divider' class='muted' text='/' /><text id='scene-name' class='muted' text='{}' /><panel id='top-spacer' style='flex-grow:1' />",escape(&project_name(editor)),escape(editor.host.store().workspace().path_of(&editor.state.doc).unwrap_or("Scene"))));
        markup.push_str("<button id='play' class='quiet' text='Play' disabled='true' description='Play requires ara-sim. The editor will launch a disposable player process when it is available.' /><button id='pause' class='quiet' text='Pause' disabled='true' description='No active play session' /><button id='step' class='quiet' text='Step' disabled='true' description='No active play session' /><panel id='top-spacer2' style='flex-grow:1' />");
        markup.push_str(&self.bind_button(
            "palette-open",
            "Commands  Ctrl K",
            Binding::Protocol("editor.palette", json!({"open":true})),
            "quiet",
        ));
        markup.push_str(&self.bind_button(
            "theme",
            if editor.state.light { "Dark" } else { "Light" },
            Binding::Protocol("editor.theme", json!({"light":!editor.state.light})),
            "quiet",
        ));
        markup.push_str("</panel><panel id='dock-root' style='flex-grow:1; min-height:0px'>");
        self.dock(&editor.state.layout.clone(), "root", editor, &mut markup)?;
        markup.push_str("</panel>");
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
        markup.push_str(&format!("<panel id='statusbar' class='statusbar'><text id='status' class='success' text='Authority connected' /><text id='status-docs' class='muted' text='{} entities' /><panel id='status-fill' style='flex-grow:1' /><text id='status-jobs' class='muted' text='{}' /><text id='status-keys' class='muted' text='Alt orbit   Middle pan   Right fly   F frame' /></panel>",self.hierarchy.items.len(),escape(&job_text)));
        let _ = running;
        if self.context {
            self.context_menu(editor, &mut markup);
        }
        if editor.state.palette {
            self.palette(editor, &mut markup)?;
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
        self.decorate_hierarchy();
        if editor.state.palette && !self.modal_open {
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
        self.paint_swatches();
        if let Some(path) = editor.state.focus_field.as_deref() {
            self.reveal_field(path)?;
        }
        Ok(())
    }
    fn reveal_field(&mut self, path: &str) -> ara_ui::Result<()> {
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
            .or_else(|| self.ui.find(&format!("edit-{id}-0")))
        {
            let key = ara_ui::source::stable_id(&self.ui.nodes[field].element.id);
            self.input.focus(&mut self.ui, key)?;
        }
        Ok(())
    }
    /// Run retained style/layout/paint work; unchanged frames return zero work counters.
    pub fn frame(&mut self) -> ara_ui::Result<ara_ui::Work> {
        let work = self.ui.frame(&mut self.text)?;
        if work.paint_nodes > 0 {
            self.paint_swatches();
        }
        Ok(work)
    }
    fn paint_swatches(&mut self) {
        for field in self.fields.values() {
            if !matches!(
                field.widget,
                crate::inspector::Widget::Vector { color: true, .. }
            ) || field.mixed
            {
                continue;
            }
            if let Some(i) = self
                .ui
                .find(&format!("swatch-{}", field.path.replace('.', "-")))
            {
                let r = self.ui.nodes[i].rect;
                let channel = |n: usize| field.value[n].as_f64().unwrap_or(1.).clamp(0., 1.);
                self.ui
                    .display
                    .commands
                    .push(ara_ui::paint::Draw::RoundedRect {
                        node: ara_ui::source::stable_id(&self.ui.nodes[i].element.id),
                        rect: Box2 {
                            x: r.x + 3.,
                            y: r.y + 3.,
                            w: (r.w - 6.).max(0.),
                            h: (r.h - 6.).max(0.),
                        },
                        radius: 3.,
                        color: ara_ui::style::Color(
                            (channel(0).powf(1. / 2.2) * 255.) as u8,
                            (channel(1).powf(1. / 2.2) * 255.) as u8,
                            (channel(2).powf(1. / 2.2) * 255.) as u8,
                            (channel(3) * 255.) as u8,
                        ),
                    });
            }
        }
    }
    pub(crate) fn poll_assets(&mut self) -> bool {
        self.browser.poll()
    }
    /// Current scene image rectangle, also the picking and spatial-input boundary.
    pub fn viewport(&self) -> Option<Box2> {
        self.ui
            .find("viewport-image")
            .map(|i| self.ui.nodes[i].rect)
    }
    /// Compact agent layout report; glyph shaping internals stay out of capture jobs.
    pub fn layout_report(&self) -> Value {
        json!({"size":self.ui.size,"focus":self.input.focused(),"nodes":self.ui.paint_order.iter().map(|i|{let n=&self.ui.nodes[*i];json!({"id":n.element.id,"stableId":ara_ui::source::stable_id(&n.element.id),"tag":n.element.tag,"box":n.rect,"clip":n.clip,"text":n.text.as_ref().map(|t|t.value.as_str()),"disabled":n.state.disabled})}).collect::<Vec<_>>()})
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
        let p = [r.x + 20., r.y + 13.];
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
