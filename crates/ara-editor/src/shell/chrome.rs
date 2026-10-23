//! The one in-app chrome row under the native title bar (D23): menus, the scene
//! breadcrumb, the Play cluster, the command search box and status chips; plus
//! the 24 px status bar. Every control runs a registry command.
use super::commands::{Target, menus::MENUS};
use super::*;

/// Approximate width of each menu's first letter at 13 px, for the mnemonic underline.
fn mnemonic_width(c: char) -> f32 {
    match c {
        'W' => 11.,
        'H' | 'V' | 'C' | 'A' => 8.,
        _ => 7.,
    }
}
impl Shell {
    pub(super) fn menubar(&mut self, editor: &mut Editor, out: &mut String) {
        out.push_str("<panel id='menubar' class='menubar'><panel id='brand' class='brand' label='Ara'><text id='brand-mark' text='a' /></panel><panel id='menu-titles' style='flex-direction:row;gap:0px;padding-left:4px'>");
        for (i, menu) in MENUS.iter().enumerate() {
            let open = matches!(&self.menu, Some(m) if m.bar() == Some(i));
            let id = format!("menu-title-{i}");
            self.bindings
                .insert(ara_ui::source::stable_id(&id), Binding::MenuBar(i));
            out.push_str(&format!("<button id='{id}' class='menu-title{}' label='{} menu' description='Alt+{}' action='Dispatch'><text id='{id}-text' text='{}' />",if open{" menu-title-open"}else{""},menu.title,menu.mnemonic,menu.title));
            if self.mnemonics {
                out.push_str(&format!("<panel id='{id}-mnemonic' class='mnemonic' style='position:absolute;left:8px;top:21px;width:{}px' />",mnemonic_width(menu.mnemonic)));
            }
            out.push_str("</button>");
        }
        let scene = editor
            .host
            .store()
            .workspace()
            .path_of(&editor.state.doc)
            .unwrap_or("Scene")
            .to_owned();
        let scene_name = scene.rsplit('/').next().unwrap_or(&scene).to_owned();
        out.push_str(&format!("</panel><panel id='crumbs' class='crumbs'><text id='crumb-project' class='crumb-strong' text='{}' />{}<text id='crumb-scene' class='crumb' text='{}' description='{}' />",escape(&project_name(editor)),icons::icon("crumb-sep","chevron-right"),escape(&scene_name),escape(&scene)));
        if self.status_note.is_none() && editor.host.store().revision() > 0 && self.unsaved(editor)
        {
            out.push_str("<panel id='crumb-dirty' class='dirty-dot' description='Unsaved changes; Ctrl+S saves now' />");
        }
        out.push_str("</panel><panel id='menubar-fill' class='fill' />");
        // The Play cluster sits centred on the window.
        let width = 216.;
        out.push_str(&format!("<panel id='play-controls' class='play-cluster{}' style='position:absolute;left:{}px;top:4px;width:{width}px'>",if editor.state.playing.is_some(){" play-live"}else{""},((self.ui.size[0]-width)*0.5).round()));
        let resume = editor.state.playing == Some(false);
        self.bind_command("play", "play.start", Target::None);
        out.push_str(&format!("<button id='play' class='ghost{}' style='height:22px;padding-left:6px;padding-right:8px' label='{}' description='{} · F5' action='Dispatch'>{}<text id='play-label' text='{}' /></button>",if editor.state.playing==Some(true){" selected"}else{""},if resume{"Resume"}else{"Play"},if resume{"Resume"}else{"Play"},icons::icon_in("play","play","tint-play"),if editor.state.playing==Some(true){"Playing"}else if resume{"Resume"}else{"Play"}));
        for (id, command, icon, label) in [
            ("pause", "play.pause", "pause", "Pause · F6"),
            ("step", "play.step", "step-forward", "Step one tick · F7"),
            ("stop", "play.stop", "square", "Stop · Shift+F5"),
        ] {
            self.bind_command(id, command, Target::None);
            let enabled = commands::get(command)
                .is_some_and(|c| c.avail(&self.ctx(editor, &Target::None)).ready());
            out.push_str(&format!("<button id='{id}' class='ghost icon-button' label='{label}' description='{label}' action='Dispatch'{}>{}</button>",if enabled{""}else{" disabled='true'"},icons::icon(id,icon)));
        }
        let mode = editor
            .state
            .settings
            .get("play.mode")
            .and_then(Value::as_str)
            .unwrap_or("local");
        let clients = editor.state.number("play.clients", 1.) as u32;
        let mode_label = match mode {
            "host" => format!("Host + {clients}"),
            "dedicated" => format!("Server + {clients}"),
            _ => "Local".into(),
        };
        self.bindings.insert(
            ara_ui::source::stable_id("play-mode"),
            Binding::ContextMenu(commands::menus::Context::Play, Target::None),
        );
        out.push_str(&format!("<panel id='play-sep' class='toolbar-sep' /><button id='play-mode' class='ghost' style='height:22px;padding-left:6px;padding-right:4px' label='Play mode' description='Play mode, clients and start options' action='Dispatch'><text id='play-mode-label' class='caption' text='{}' />{}</button></panel>",escape(&mode_label),icons::icon("play-mode","chevron-down")));
        self.bind_command("search-box", "edit.palette", Target::None);
        out.push_str(&format!("<button id='search-box' class='search-box' label='Search or run a command' description='Search commands, entities and assets' action='Dispatch'>{}<text id='search-box-text' class='muted' text='Search or run a command…' /><panel id='search-box-fill' class='fill' />{}</button>",icons::icon("search-box","search"),self.keycaps("search-box","edit.palette")));
        // Status chips: each opens the panel it summarizes.
        let logs = editor
            .action("logs", json!({"limit": 300}))
            .unwrap_or(Value::Null);
        let (mut errors, mut warnings) = (0, 0);
        for entry in logs["entries"].as_array().into_iter().flatten() {
            match entry["diagnostic"]["severity"].as_str() {
                Some("error") => errors += 1,
                Some("warning") => warnings += 1,
                _ => {}
            }
        }
        let agent_changes =
            editor.unreviewed_ops().count() + editor.pending().as_array().map_or(0, Vec::len);
        out.push_str("<panel id='chips' style='flex-direction:row;align-items:center;gap:2px;padding-left:6px'>");
        if errors + warnings > 0 {
            self.bind_command("chip-problems", "window.console", Target::None);
            out.push_str(&format!("<button id='chip-problems' class='chip' label='Problems' description='{errors} errors, {warnings} warnings · open Console' action='Dispatch'>{}<text id='chip-problems-text' class='{}' text='{}' /></button>",icons::icon_in("chip-problems","triangle-alert",if errors>0{"destructive"}else{"warning"}),if errors>0{"destructive"}else{"warning"},errors+warnings));
        }
        if agent_changes > 0 {
            self.bind_command("chip-agents", "window.agents", Target::None);
            out.push_str(&format!("<button id='chip-agents' class='chip' label='Agent changes' description='{agent_changes} agent changes to review · open Agents' action='Dispatch'>{}<text id='chip-agents-text' class='warning' text='{agent_changes}' /></button>",icons::icon_in("chip-agents","bot","warning")));
        }
        self.bind_command("chip-settings", "edit.preferences", Target::None);
        out.push_str(&format!("<button id='chip-settings' class='ghost icon-button' label='Preferences' description='Preferences · Ctrl+,' action='Dispatch'>{}</button></panel></panel>",icons::icon("chip-settings","settings-2")));
    }
    pub(super) fn statusbar(&mut self, editor: &mut Editor, out: &mut String) {
        let jobs = editor.action("jobs.list", json!({})).unwrap_or(Value::Null);
        let running = jobs["jobs"]
            .as_array()
            .and_then(|j| j.iter().find(|j| j["status"] == "running"))
            .map(|j| {
                format!(
                    "{} {:0.0}%",
                    j["name"].as_str().unwrap_or("Job"),
                    j["progress"].as_f64().unwrap_or(0.) * 100.
                )
            });
        let hint = match editor.state.focus.as_str() {
            "material" | "animation" => "Space Add node · F Frame · drag pins to connect",
            "vfx" => "Click a module to edit it · right-click for actions",
            "settings" => "Type to search · Ctrl+Z undoes setting changes",
            _ => "W E R tools · F Frame · Alt+drag Orbit · RMB+WASD Fly · Ctrl+K Commands",
        };
        let saved = self.status_note.clone().unwrap_or_else(|| {
            if self.unsaved(editor) {
                "Saving…".into()
            } else {
                "All changes saved".into()
            }
        });
        self.bind_command("status-entities", "window.hierarchy", Target::None);
        out.push_str(&format!("<panel id='statusbar' class='statusbar'><panel id='connection-dot' class='status-dot' /><text id='status' class='caption' text='Connected' style='padding-right:8px' /><button id='status-entities' class='chip' label='Entities' action='Dispatch'>{}<text id='status-docs' text='{} entities{}' /></button><text id='status-saved' class='caption' text='{}' style='padding-left:8px' /><panel id='status-fill' class='fill' /><text id='status-hint' class='caption' text='{}' style='padding-right:12px' />",
            icons::icon("status-entities","box"),
            self.hierarchy.items.len(),
            if editor.state.selection.is_empty(){String::new()}else{format!(" · {} selected",editor.state.selection.len())},
            escape(&saved),
            escape(hint)));
        self.bind_command("status-jobs", "window.assets", Target::None);
        out.push_str(&format!("<button id='status-jobs' class='chip' label='Jobs' action='Dispatch'>{}<text id='status-jobs-text' text='{}' /></button>",icons::icon("status-jobs",if running.is_some(){"activity"}else{"circle-check"}),escape(running.as_deref().unwrap_or("Jobs idle"))));
        self.bind_command("status-frame", "window.profiler", Target::None);
        out.push_str(&format!("<button id='status-frame' class='chip' label='Frame time' description='Open the Profiler' action='Dispatch'>{}<text id='status-timing' text='144 FPS · 6.94 ms' /></button></panel>",icons::icon("status-frame","gauge")));
    }
    /// Keycap chips for a command's primary binding.
    pub(super) fn keycaps(&self, id: &str, command: &str) -> String {
        let Some(chord) = self.keymap.chords(command).first() else {
            return String::new();
        };
        let mut out = format!("<panel id='{id}-keys' class='keys'>");
        for (n, cap) in chord.caps().iter().enumerate() {
            out.push_str(&format!("<panel id='{id}-key-{n}' class='keycap'><text id='{id}-key-text-{n}' text='{}' /></panel>",escape(cap)));
        }
        out.push_str("</panel>");
        out
    }
    fn unsaved(&self, editor: &Editor) -> bool {
        editor.host.unsaved()
    }
}
