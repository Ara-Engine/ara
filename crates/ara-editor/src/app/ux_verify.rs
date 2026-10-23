//! Editor UX v2 capture route (`--verify-ux`): menus, context menus, palette,
//! Settings, a floating Inspector window and every tool tab, in both themes,
//! through the production window loop. It writes PNGs, layout reports, the
//! registry export and a viewport-share report; timings come from the loop.
use super::*;
use crate::shell::commands::{Target, menus::Context};

/// What a step shows before its capture.
#[derive(Clone, Copy)]
enum Setup {
    Main,
    Menu(usize, Option<usize>),
    Context(Context),
    Palette(&'static str),
    Settings(&'static str),
    Float(&'static str),
    Tab(&'static str),
    Empty,
}
const STEPS: &[(&str, Setup)] = &[
    ("main", Setup::Main),
    ("menu-file", Setup::Menu(0, None)),
    ("menu-edit", Setup::Menu(1, Some(23))),
    ("menu-view", Setup::Menu(2, Some(1))),
    ("menu-create", Setup::Menu(3, Some(5))),
    ("menu-window", Setup::Menu(7, Some(0))),
    ("context-hierarchy", Setup::Context(Context::Entity)),
    ("context-viewport", Setup::Context(Context::Viewport)),
    ("context-field", Setup::Context(Context::Field)),
    ("context-tab", Setup::Context(Context::Tab)),
    ("palette", Setup::Palette("fra")),
    ("palette-entities", Setup::Palette("@")),
    ("settings", Setup::Settings("")),
    ("shortcuts", Setup::Settings("Keyboard shortcuts")),
    ("float-inspector", Setup::Float("inspector")),
    ("tab-material", Setup::Tab("material")),
    ("tab-vfx", Setup::Tab("vfx")),
    ("tab-animation", Setup::Tab("animation")),
    ("tab-profiler", Setup::Tab("profiler")),
    ("tab-game", Setup::Tab("game")),
    ("tab-console", Setup::Tab("console")),
    ("tab-agents", Setup::Tab("agents")),
    ("tab-assets", Setup::Tab("assets")),
    ("empty-inspector", Setup::Empty),
];
pub(super) struct UxVerification {
    step: usize,
    age: u32,
    output: PathBuf,
    report: Vec<Value>,
    pub finished_at: Option<Instant>,
    /// Material, effect and controller documents opened by the tool-tab steps.
    resources: [String; 3],
}
impl UxVerification {
    pub fn new(editor: &mut Editor, output: &std::path::Path) -> ara_ui::Result<Self> {
        editor.action("editor.preferences", json!({"reduceMotion": true}))?;
        editor.action("editor.theme", json!({"light": false}))?;
        editor.action("editor.layout.reset", json!({}))?;
        // Real documents for the tool tabs: a material, an effect and a controller.
        let material = super::panels_verify::resource(editor, "material")?;
        let vfx = super::panels_verify::resource(editor, "vfx")?;
        let animation = super::panels_verify::resource(editor, "animation")?;
        let controller = json!({"initial":"Idle","parameters":{"speed":{}},"nodes":{"idle":{"clip":null},"run":{"clip":null},"locomotion":{"kind":"blend1d","parameterX":"speed","samples":[{"node":"idle","x":0},{"node":"run","x":1}]}},"states":{"Idle":{"node":"idle","duration":48000},"Run":{"node":"run","duration":24000},"Locomotion":{"node":"locomotion","duration":48000}},"transitions":[{"id":"idle_to_run","from":"Idle","to":"Run","duration":4800,"conditions":[{"parameter":"speed","comparison":"greater","floatValue":0.2}]},{"id":"run_to_idle","from":"Run","to":"Idle","duration":4800}]});
        editor.apply_to(&animation, vec![json!({"cmd":"editor.animation.controller.set","args":{"record":"an_controller","controller":controller}})], "Prepare UX verification controller")?;
        editor.action("profile.start", json!({}))?;
        std::fs::create_dir_all(output)?;
        Ok(Self {
            step: 0,
            age: 0,
            output: output.into(),
            report: Vec::new(),
            finished_at: None,
            resources: [material, vfx, animation],
        })
    }
    pub fn finished(&self) -> bool {
        self.step >= STEPS.len()
    }
    /// Advance one frame; returns whether the UI changed.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui: &mut UiRenderer,
        renderer: &mut Renderer,
        floats: &windows::Floats,
    ) -> ara_ui::Result<bool> {
        if self.finished() {
            return Ok(false);
        }
        let (name, setup) = STEPS[self.step];
        self.age += 1;
        if self.age == 1 {
            self.setup(editor, shell, setup)?;
            return Ok(true);
        }
        // Floating windows and thumbnails need a few frames to open and settle.
        let wait = match setup {
            Setup::Float(_) | Setup::Settings(_) => 24,
            Setup::Tab("material" | "vfx" | "assets") => 90,
            _ => 8,
        };
        if self.age < wait {
            return Ok(false);
        }
        let float = match setup {
            Setup::Float(panel) | Setup::Settings(panel) => {
                let panel = if panel.is_empty() || panel == "Keyboard shortcuts" {
                    "settings"
                } else {
                    panel
                };
                floats.windows.iter().find(|f| {
                    editor.state.floating.iter().any(|w| {
                        w.id == f.id && super::panels_of(&w.root).iter().any(|p| p == panel)
                    })
                })
            }
            _ => None,
        };
        let capture = super::verify::Verification::new(&self.output);
        let viewport = shell
            .viewport()
            .map(|r| r.w * r.h / (shell.ui.size[0] * shell.ui.size[1]));
        for (theme, light) in [("dark", false), ("light", true)] {
            editor.action("editor.theme", json!({"light": light}))?;
            shell.rebuild(editor)?;
            self.reopen(editor, shell, setup)?;
            shell.upload_thumbnails(ui)?;
            capture.screenshot(&format!("{name}-{theme}"), editor, shell, gpu, ui, renderer)?;
            if let Some(float) = float {
                let work = shell.frame_surface(&float.id)?;
                let _ = work;
                let surface = shell.surface(&float.id).ok_or("floating surface")?;
                gpu.select_target(float.key);
                ui.render(
                    gpu,
                    &surface.ui.display,
                    &shell.text.assets,
                    surface.ui.size.map(|v| v.round() as u32),
                    TextMode::Raster,
                )?;
                gpu.pixels()?
                    .save(self.output.join(format!("{name}-window-{theme}.png")))?;
                gpu.select_target(0);
            }
        }
        editor.action("editor.theme", json!({"light": false}))?;
        self.report.push(json!({
            "step": name,
            "viewportShare": viewport,
            "floatingWindows": editor.state.floating.len(),
            // Profiler frame bars are data rows (they select a frame), not toolbar buttons.
            "uncommandedToolbarButtons": shell
                .uncommanded_buttons(&["tool-", "snap-", "view-", "play", "pause", "step", "stop", "search-box", "chip-", "status-", "graph-", "profile-", "asset-import", "panel-menu-", "menu-title-", "m0-", "m1-", "m2-"])
                .into_iter()
                .filter(|id| !id.starts_with("profile-frame-"))
                .collect::<Vec<_>>(),
        }));
        self.teardown(editor, shell, setup)?;
        self.step += 1;
        self.age = 0;
        if self.finished() {
            self.write(editor, shell)?;
            self.finished_at = Some(Instant::now());
        }
        Ok(true)
    }
    fn setup(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        setup: Setup,
    ) -> ara_ui::Result<()> {
        let crate_entity = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?
            .iter()
            .find(|(_, e)| e.record.get("name").and_then(Value::as_str) == Some("Salvage crate"))
            .map(|(id, _)| id.clone());
        let select = crate_entity.clone().map(|e| vec![e]).unwrap_or_default();
        editor.action("editor.select", json!({"entities": select}))?;
        match setup {
            Setup::Tab(panel) => {
                let resource = match panel {
                    "material" => Some(&self.resources[0]),
                    "vfx" => Some(&self.resources[1]),
                    "animation" => Some(&self.resources[2]),
                    _ => None,
                };
                if let Some(doc) = resource {
                    editor.action("editor.workspace", json!({"resource": doc}))?;
                }
                if panel == "vfx" {
                    // Show a module's parameters and its row actions.
                    let graph = ara_graph::Graph::from_document(
                        editor
                            .host
                            .store()
                            .workspace()
                            .doc(&self.resources[1])
                            .ok_or("effect")?,
                    )
                    .map_err(|d| d.to_string())?;
                    let effect = ara_vfx::Effect::from_graph(&graph).map_err(|d| d.to_string())?;
                    if let Some(emitter) = effect.emitters.first()
                        && let Some(module) = emitter.stacks.update.first()
                    {
                        editor.action(
                            "editor.workspace",
                            json!({"item": format!("{}|update|{}", emitter.name, module.name)}),
                        )?;
                    }
                }
                if panel == "profiler" && editor.profile_current.is_none() {
                    editor.action("editor.profile.freeze", json!({}))?;
                }
                editor.action("editor.focus", json!({"panel": panel}))?;
            }
            Setup::Float(panel) => {
                let m = shell.main_rect;
                let rect = [m[0] + m[2] - 520, m[1] + 80, 440, 620];
                editor.action(
                    "editor.layout.move",
                    json!({"panel": panel, "slot": "float", "rect": rect.map(|v| v as f32)}),
                )?;
            }
            Setup::Settings(category) => {
                editor.action(
                    "editor.settings.view",
                    json!({"scope": "user", "category": category}),
                )?;
                let rect = shell.float_rect([900, 600]);
                editor.action(
                    "editor.layout.show",
                    json!({"panel": "settings", "rect": rect.map(|v| v as f32)}),
                )?;
            }
            Setup::Empty => {
                editor.action("editor.select", json!({"entities": []}))?;
            }
            _ => {}
        }
        shell.rebuild(editor)?;
        self.reopen(editor, shell, setup)
    }
    /// Menus, palettes and context menus are transient; reopen them after a theme rebuild.
    fn reopen(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        setup: Setup,
    ) -> ara_ui::Result<()> {
        match setup {
            Setup::Menu(index, sub) => {
                shell.open_bar(index, false);
                if let (Some(sub), Some(menu)) = (sub, shell.menu.as_mut()) {
                    menu.open.push(sub);
                }
                shell.rebuild(editor)?;
            }
            Setup::Context(kind) => {
                let point = match kind {
                    Context::Entity => shell.ui.find("hierarchy-rows").map(|i| {
                        let r = shell.ui.nodes[i].rect;
                        [r.x + 90., r.y + 22. * 2.5]
                    }),
                    Context::Field => shell.ui.find("field-Transform-position").map(|i| {
                        let r = shell.ui.nodes[i].rect;
                        [r.x + 30., r.y + r.h * 0.5]
                    }),
                    Context::Tab => shell.ui.find("tab-scene").map(|i| {
                        let r = shell.ui.nodes[i].rect;
                        [r.x + r.w * 0.5, r.y + r.h * 0.5]
                    }),
                    _ => shell
                        .viewport()
                        .map(|r| [r.x + r.w * 0.3, r.y + r.h * 0.75]),
                }
                .ok_or("context target not visible")?;
                shell.pointer = point;
                if kind == Context::Viewport {
                    shell.open_viewport_menu(editor, Context::Viewport, Target::None)?;
                } else if !shell.open_context(editor)? {
                    return Err(format!("no context menu for {kind:?}").into());
                }
            }
            Setup::Palette(query) => {
                editor.action("editor.search", json!({"panel": "palette", "query": query}))?;
                editor.action("editor.palette", json!({"open": true}))?;
                shell.rebuild(editor)?;
            }
            _ => {}
        }
        Ok(())
    }
    fn teardown(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        setup: Setup,
    ) -> ara_ui::Result<()> {
        shell.menu = None;
        if editor.state.palette {
            editor.action("editor.palette", json!({"open": false}))?;
        }
        match setup {
            Setup::Float(_) | Setup::Settings(_) | Setup::Tab(_) => {
                editor.action("editor.layout.reset", json!({}))?;
            }
            _ => {}
        }
        shell.rebuild(editor)
    }
    fn write(&self, editor: &mut Editor, shell: &Shell) -> ara_ui::Result<()> {
        let commands = editor.action("editor.commands", json!({}))?;
        let list = commands["commands"].as_array().cloned().unwrap_or_default();
        let enabled = list.iter().filter(|c| c["enabled"] == true).count();
        std::fs::write(
            self.output.join("commands.json"),
            ara_json::to_canonical_string(&commands),
        )?;
        std::fs::write(
            self.output.join("ux-report.json"),
            ara_json::to_canonical_string(&json!({
                "size": shell.ui.size,
                "steps": self.report,
                "commands": {"total": list.len(), "enabled": enabled, "disabledWithReason": list.len() - enabled},
            })),
        )?;
        Ok(())
    }
}
