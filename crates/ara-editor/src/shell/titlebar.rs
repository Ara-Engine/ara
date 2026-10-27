//! One custom title bar: menus, Play and native window controls.
use super::commands::{Target, menus::MENUS};
use super::*;
#[derive(Hash)]
struct TitlebarState {
    open: Option<usize>,
    mnemonics: bool,
    project: String,
    scene: String,
    dirty: bool,
    playing: Option<bool>,
    starting: bool,
    start_paused: bool,
    enabled: [bool; 3],
    mode_label: String,
    maximized: bool,
}
impl Shell {
    /// Memoized caption markup; the same command registry drives menus, keys and MCP.
    pub(super) fn menubar(&mut self, editor: &mut Editor, out: &mut String) {
        let scene = editor
            .host
            .store()
            .workspace()
            .path_of(&editor.state.doc)
            .unwrap_or("Scene")
            .to_owned();
        let ctx = self.ctx(editor, &Target::None);
        let enabled = ["play.pause", "play.step", "play.stop"]
            .map(|command| commands::get(command).is_some_and(|c| c.avail(&ctx).ready()));
        let starting = ctx.starting;
        let mode = editor
            .state
            .settings
            .get("play.mode")
            .and_then(Value::as_str)
            .unwrap_or("local");
        let clients = editor.state.number("play.clients", 1.) as u32;
        let state = TitlebarState {
            open: self.menu.as_ref().and_then(|m| m.bar()),
            mnemonics: self.mnemonics,
            project: project_name(editor),
            scene,
            dirty: self.status_note.is_none()
                && editor.host.store().revision() > 0
                && self.unsaved(editor),
            playing: editor.state.playing,
            starting,
            start_paused: editor
                .state
                .settings
                .get("play.startPaused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            enabled,
            maximized: self.native_maximized,
            mode_label: match mode {
                "host" => format!("Host + {clients}"),
                "dedicated" => format!("Server + {clients}"),
                _ => "Local".into(),
            },
        };
        self.memoized("menubar", memo::digest(&state), out, |shell, out| {
            shell.titlebar_markup(&state, out)
        });
    }
    fn titlebar_markup(&mut self, s: &TitlebarState, out: &mut String) {
        self.bind_command("brand", "help.about", Target::None);
        out.push_str("<panel id='menubar' class='menubar' label='Title bar'><panel id='titlebar-left' class='titlebar-side'><button id='brand' class='brand' label='About Ara' description='About Ara' action='Dispatch'><image id='brand-logo' class='brand-logo' src='brand-logo' /></button><panel id='menu-titles' class='titlebar-menus'>");
        for (i, menu) in MENUS.iter().enumerate() {
            let id = format!("menu-title-{i}");
            self.bindings
                .insert(ara_ui::source::stable_id(&id), Binding::MenuBar(i));
            out.push_str(&format!("<button id='{id}' class='menu-title{}' label='{} menu' description='Alt+{}' action='Dispatch'><text id='{id}-text' text='{}' />",if s.open == Some(i) { " menu-title-open" } else { "" },menu.title,menu.mnemonic,menu.title));
            if s.mnemonics {
                let width = match menu.mnemonic {
                    'W' => 10.,
                    'H' | 'V' | 'C' => 8.,
                    _ => 7.,
                };
                out.push_str(&format!("<panel id='{id}-mnemonic' class='mnemonic' style='position:absolute;left:6px;top:21px;width:{width}px' />"));
            }
            out.push_str("</button>");
        }
        out.push_str("</panel></panel><panel id='titlebar-center' class='titlebar-center'>");
        self.titlebar_play(s, out);
        out.push_str("</panel><panel id='titlebar-right' class='titlebar-side titlebar-right'>");
        out.push_str(&format!("<panel id='crumbs' class='titlebar-path' description='{} · {}'><text id='crumb-project' class='titlebar-project' text='{}' /><text id='crumb-sep' class='muted' text='·' /><text id='crumb-scene' class='titlebar-scene' text='{}' description='{}' />",escape(&s.project),escape(&s.scene),escape(&s.project),escape(&s.scene),escape(&s.scene)));
        if s.dirty {
            out.push_str("<panel id='crumb-dirty' class='dirty-dot' description='Unsaved changes; Ctrl+S saves now' />");
        }
        out.push_str("</panel><panel id='caption-buttons' class='caption-buttons'>");
        for (id, command, label, glyph) in [
            ("window-minimize", "window.minimize", "Minimize", "−"),
            (
                "window-maximize",
                "window.maximize_window",
                if s.maximized { "Restore" } else { "Maximize" },
                if s.maximized { "▣" } else { "□" },
            ),
            ("window-close", "file.exit", "Close", "×"),
        ] {
            self.bind_command(id, command, Target::None);
            out.push_str(&format!("<button id='{id}' class='caption-button{}' label='{label}' description='{label}' action='Dispatch'><text id='{id}-glyph' class='caption-glyph' text='{glyph}' /></button>",if id == "window-close" { " caption-close" } else { "" }));
        }
        out.push_str("</panel></panel></panel>");
    }
    fn titlebar_play(&mut self, s: &TitlebarState, out: &mut String) {
        out.push_str(&format!(
            "<panel id='play-controls' class='play-cluster{}'>",
            if s.playing.is_some() {
                " play-live"
            } else {
                ""
            }
        ));
        let [pause_ready, step_ready, stop_ready] = s.enabled;
        if s.playing.is_none() && !s.starting {
            self.bind_command("play", "play.start", Target::None);
            out.push_str(&format!("<button id='play' class='ghost' style='height:22px;padding-left:6px;padding-right:8px' label='Play' description='Play · F5' action='Dispatch'>{}<text id='play-label' text='Play' /></button>",icons::icon_in("play","play","tint-play")));
            self.bind_command("pause", "play.start_paused", Target::None);
            out.push_str(&format!("<button id='pause' class='ghost icon-button{}' label='Start paused' description='Start Play paused at tick 0' action='Dispatch'>{}</button>",if s.start_paused { " selected" } else { "" },icons::icon("pause","pause")));
        } else {
            self.bind_command("stop", "play.stop", Target::None);
            let what = if s.starting {
                "Cancel the starting Play"
            } else {
                "Stop"
            };
            out.push_str(&format!("<button id='stop' class='ghost selected' style='height:22px;padding-left:6px;padding-right:8px' label='Stop' description='{what} · Shift+F5' action='Dispatch'{}>{}<text id='stop-label' text='Stop' /></button>",if stop_ready { "" } else { " disabled='true'" },icons::icon_in("stop","square","tint-play")));
            let (command, icon, label, ready) = if s.playing == Some(false) {
                ("play.start", "play", "Resume · F5", true)
            } else {
                ("play.pause", "pause", "Pause · F6", pause_ready)
            };
            self.bind_command("pause", command, Target::None);
            out.push_str(&format!("<button id='pause' class='ghost icon-button' label='{label}' description='{label}' action='Dispatch'{}>{}</button>",if ready { "" } else { " disabled='true'" },icons::icon("pause",icon)));
        }
        self.bind_command("step", "play.step", Target::None);
        out.push_str(&format!("<button id='step' class='ghost icon-button' label='Step one tick · F7' description='Step one tick · F7' action='Dispatch'{}>{}</button>",if step_ready { "" } else { " disabled='true'" },icons::icon("step","step-forward")));
        self.bindings.insert(
            ara_ui::source::stable_id("play-mode"),
            Binding::ContextMenu(commands::menus::Context::Play, Target::None),
        );
        out.push_str(&format!("<panel id='play-sep' class='toolbar-sep' /><button id='play-mode' class='ghost icon-button' label='Play mode' description='{} · Play mode, clients and start options' action='Dispatch'>{}</button></panel>",escape(&s.mode_label),icons::icon("play-mode","chevron-down")));
    }
    /// Reuse native hit-test storage after layout; all units passed to SDL are window points.
    pub(crate) fn update_chrome(
        &self,
        window: &mut ara_platform::Window,
        dpi: f32,
    ) -> Option<ara_ui::paint::Box2> {
        let scale = 1. / crate::native::points_to_logical(window.raw(), dpi);
        let rect = |id: &str| {
            self.shown_node(id).map(|i| {
                let r = self.ui.nodes[i].rect;
                [r.x * scale, r.y * scale, r.w * scale, r.h * scale]
            })
        };
        let header = rect("menubar")?;
        let caption_border = if window.maximized() { 0. } else { 4. * scale };
        let chrome = window.chrome_mut()?;
        chrome.height = header[3];
        chrome.border = 4. * scale;
        chrome.controls.clear();
        let popup_open = self.ui.popover_nodes().any(|i| {
            let element = &self.ui.nodes[i].element;
            element.id != "tooltip"
                && element.attrs.get("role").map(String::as_str) != Some("tooltip")
        });
        if popup_open
            || self.menu.is_some()
            || self.dialog.is_some()
            || self.ui.find("palette").is_some_and(|i| self.ui.shown(i))
        {
            chrome
                .controls
                .push([0., 0., self.ui.size[0] * scale, header[3]]);
        } else {
            for id in ["brand", "menu-titles", "play-controls", "caption-buttons"] {
                if let Some(r) = rect(id) {
                    chrome.controls.push(r);
                }
            }
        }
        rect("window-maximize").map(|r| ara_ui::paint::Box2 {
            x: r[0],
            y: r[1] + caption_border,
            w: r[2],
            h: (r[3] - caption_border).max(0.),
        })
    }
}
