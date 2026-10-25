//! The one in-app chrome row under the native title bar (D23): menus, the scene
//! breadcrumb, the Play cluster, the command search box and status chips; plus
//! the 24 px status bar. Every control runs a registry command.
use super::commands::{
    Target,
    menus::{Context, MENUS},
};
use super::*;

/// Everything the menubar shows: the digest it is memoized on and its markup's only
/// input (besides the keymap behind the search box's key caps, `keymap`).
#[derive(Hash)]
struct MenubarState {
    /// The menu whose title is open.
    open: Option<usize>,
    /// Alt held: mnemonic underlines.
    mnemonics: bool,
    project: String,
    /// The open scene's path.
    scene: String,
    /// Unsaved changes (the breadcrumb's dot).
    dirty: bool,
    playing: Option<bool>,
    /// Play was started and is not live yet (Stop cancels it).
    starting: bool,
    /// Play starts paused (`play.startPaused`).
    start_paused: bool,
    /// Pause, Step and Stop availability.
    enabled: [bool; 3],
    mode_label: String,
    /// `Shell::keymap_revision`.
    keymap: u64,
    errors: u64,
    warnings: u64,
    agents: usize,
    /// An agent edited just now.
    active: bool,
}

/// Approximate width of each menu's first letter at 13 px, for the mnemonic underline.
fn mnemonic_width(c: char) -> f32 {
    match c {
        'W' => 11.,
        'H' | 'V' | 'C' | 'A' => 8.,
        _ => 7.,
    }
}
impl Shell {
    /// The menu titles, breadcrumb, Play cluster, search box and chips, memoized on
    /// everything they show ([`MenubarState`]).
    pub(super) fn menubar(&mut self, editor: &mut Editor, out: &mut String) {
        let state = self.menubar_state(editor);
        let key = memo::digest(&state);
        self.memoized("menubar", key, out, |shell, out| {
            shell.menubar_markup(&state, out);
        });
    }
    /// What the menubar shows, read once per rebuild (the problem counts query the log).
    fn menubar_state(&self, editor: &mut Editor) -> MenubarState {
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
        let mode_label = match mode {
            "host" => format!("Host + {clients}"),
            "dedicated" => format!("Server + {clients}"),
            _ => "Local".into(),
        };
        let dirty = self.status_note.is_none()
            && editor.host.store().revision() > 0
            && self.unsaved(editor);
        // The counts are the log's totals (each query returns one entry), not a scan of
        // fetched entries: a long session's log holds hundreds.
        let mut total = |severity: &str, codes: Option<&[&str]>| {
            let mut query = json!({"severity": severity, "group": false, "limit": 1});
            if let Some(codes) = codes {
                query["codes"] = json!(codes);
            }
            editor
                .action("logs", query)
                .ok()
                .and_then(|logs| logs["total"].as_u64())
                .unwrap_or(0)
        };
        // This machine's Vulkan loader and overlay layers (OBS, Overwolf hooks) are not the
        // project's problems: Problems and Console list them, the chip does not count them.
        let host: &[&str] = &["ARA_GPU_ENVIRONMENT"];
        let mut count =
            |severity: &str| total(severity, None).saturating_sub(total(severity, Some(host)));
        let errors = count("error");
        let warnings = count("warning").saturating_sub(errors);
        MenubarState {
            open: self.menu.as_ref().and_then(|m| m.bar()),
            mnemonics: self.mnemonics,
            project: project_name(editor),
            scene,
            dirty,
            playing: editor.state.playing,
            starting,
            start_paused: editor
                .state
                .settings
                .get("play.startPaused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            enabled,
            mode_label,
            keymap: self.keymap_revision,
            errors,
            warnings,
            // D24: agents present (and whether one edited just now), not a review count.
            agents: editor.state.presence.len(),
            active: editor.activity.agent_active(std::time::Instant::now()),
        }
    }
    fn menubar_markup(&mut self, s: &MenubarState, out: &mut String) {
        out.push_str("<panel id='menubar' class='menubar'><panel id='brand' class='brand' label='Ara'><image id='brand-logo' class='brand-logo' src='brand-logo' /></panel><panel id='menu-titles' style='flex-direction:row;gap:0px;padding-left:4px'>");
        for (i, menu) in MENUS.iter().enumerate() {
            let open = s.open == Some(i);
            let id = format!("menu-title-{i}");
            self.bindings
                .insert(ara_ui::source::stable_id(&id), Binding::MenuBar(i));
            out.push_str(&format!("<button id='{id}' class='menu-title{}' label='{} menu' description='Alt+{}' action='Dispatch'><text id='{id}-text' text='{}' />",if open{" menu-title-open"}else{""},menu.title,menu.mnemonic,menu.title));
            if s.mnemonics {
                out.push_str(&format!("<panel id='{id}-mnemonic' class='mnemonic' style='position:absolute;left:8px;top:21px;width:{}px' />",mnemonic_width(menu.mnemonic)));
            }
            out.push_str("</button>");
        }
        // The scene's name as people read it; the tooltip has its project path.
        let scene_name = names::stem(names::file_name(&s.scene));
        out.push_str(&format!("</panel><panel id='crumbs' class='crumbs'><text id='crumb-project' class='crumb-strong' text='{}' />{}<text id='crumb-scene' class='crumb' text='{}' description='{}' />",escape(&s.project),icons::icon("crumb-sep","chevron-right"),escape(scene_name),escape(&s.scene)));
        if s.dirty {
            out.push_str("<panel id='crumb-dirty' class='dirty-dot' description='Unsaved changes; Ctrl+S saves now' />");
        }
        out.push_str("</panel><panel id='menubar-fill' class='fill' />");
        // The Play cluster sits between flexible gaps: centred in the free space.
        out.push_str(&format!(
            "<panel id='play-controls' class='play-cluster{}'>",
            if s.playing.is_some() {
                " play-live"
            } else {
                ""
            }
        ));
        // Idle: Play, and a toggle to start paused. Starting or playing: Play becomes
        // Stop (it also cancels a start), then Pause or Resume, and Step.
        let [pause_ready, step_ready, stop_ready] = s.enabled;
        if s.playing.is_none() && !s.starting {
            self.bind_command("play", "play.start", Target::None);
            out.push_str(&format!("<button id='play' class='ghost' style='height:22px;padding-left:6px;padding-right:8px' label='Play' description='{} · F5' action='Dispatch'>{}<text id='play-label' text='Play' /></button>",if s.start_paused{"Play, paused at tick 0"}else{"Play"},icons::icon_in("play","play","tint-play")));
            self.bind_command("pause", "play.start_paused", Target::None);
            out.push_str(&format!("<button id='pause' class='ghost icon-button{}' label='Start paused' description='{}' action='Dispatch'>{}</button>",if s.start_paused{" selected"}else{""},if s.start_paused{"Play starts paused at tick 0 · click to start running"}else{"Start Play paused at tick 0"},icons::icon("pause","pause")));
        } else {
            self.bind_command("stop", "play.stop", Target::None);
            let what = if s.starting {
                "Cancel the starting Play"
            } else {
                "Stop"
            };
            out.push_str(&format!("<button id='stop' class='ghost selected' style='height:22px;padding-left:6px;padding-right:8px' label='Stop' description='{what} · Shift+F5' action='Dispatch'{}>{}<text id='stop-label' text='Stop' /></button>",if stop_ready{""}else{" disabled='true'"},icons::icon_in("stop","square","tint-play")));
            let paused = s.playing == Some(false);
            let (command, icon, label, ready) = if paused {
                ("play.start", "play", "Resume · F5", true)
            } else {
                ("play.pause", "pause", "Pause · F6", pause_ready)
            };
            self.bind_command("pause", command, Target::None);
            out.push_str(&format!("<button id='pause' class='ghost icon-button' label='{label}' description='{label}' action='Dispatch'{}>{}</button>",if ready{""}else{" disabled='true'"},icons::icon("pause",icon)));
            self.bind_command("step", "play.step", Target::None);
            out.push_str(&format!("<button id='step' class='ghost icon-button' label='Step one tick · F7' description='Step one tick · F7' action='Dispatch'{}>{}</button>",if step_ready{""}else{" disabled='true'"},icons::icon("step","step-forward")));
        }
        let mode_label = &s.mode_label;
        self.bindings.insert(
            ara_ui::source::stable_id("play-mode"),
            Binding::ContextMenu(commands::menus::Context::Play, Target::None),
        );
        out.push_str(&format!("<panel id='play-sep' class='toolbar-sep' /><button id='play-mode' class='ghost' style='height:22px;padding-left:6px;padding-right:4px' label='Play mode' description='Play mode, clients and start options' action='Dispatch'><text id='play-mode-label' class='caption' text='{}' />{}</button></panel><panel id='menubar-fill-2' class='fill' />",escape(mode_label),icons::icon("play-mode","chevron-down")));
        self.bind_command("search-box", "edit.palette", Target::None);
        out.push_str(&format!("<button id='search-box' class='search-box' label='Search or run a command' description='Search commands, entities and assets' action='Dispatch'>{}<text id='search-box-text' class='muted' text='Search or run a command…' /><panel id='search-box-fill' class='fill' />{}</button>",icons::icon("search-box","search"),self.keycaps("search-box","edit.palette")));
        let (errors, warnings, agents, active) = (s.errors, s.warnings, s.agents, s.active);
        // Status chips: each opens the panel it summarizes.
        out.push_str("<panel id='chips' style='flex-direction:row;align-items:center;gap:2px;padding-left:6px'>");
        if errors + warnings > 0 {
            self.bind_command("chip-problems", "window.problems", Target::None);
            out.push_str(&format!("<button id='chip-problems' class='chip' label='Problems' description='{errors} errors, {warnings} warnings · open Problems' action='Dispatch'>{}<text id='chip-problems-text' class='{}' text='{}' /></button>",icons::icon_in("chip-problems","triangle-alert",if errors>0{"destructive"}else{"warning"}),if errors>0{"destructive"}else{"warning"},errors+warnings));
        }
        if agents > 0 || active {
            self.bind_command("chip-agents", "window.agents", Target::None);
            let tint = if active { "accent" } else { "muted" };
            out.push_str(&format!("<button id='chip-agents' class='chip' label='Agents' description='{agents} agents connected{} · open the activity feed' action='Dispatch'>{}<text id='chip-agents-text' class='{tint}' text='{}' /></button>",if active{" · editing now"}else{""},icons::icon_in("chip-agents","bot",tint),agents.max(1)));
        }
        self.bind_command("chip-settings", "edit.preferences", Target::None);
        out.push_str(&format!("<button id='chip-settings' class='ghost icon-button' label='Preferences' description='Preferences · Ctrl+,' action='Dispatch'>{}</button></panel></panel>",icons::icon("chip-settings","settings-2")));
    }
    /// The status bar: connection and agents, Play state, job progress (imports,
    /// cooks, script builds), save state, the focused panel's hint and frame time.
    pub(super) fn statusbar(&mut self, editor: &mut Editor, out: &mut String) {
        let jobs = editor.action("jobs.list", json!({})).unwrap_or(Value::Null);
        let active: Vec<&Value> = jobs["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|j| matches!(j["status"].as_str(), Some("running" | "queued")))
            .collect();
        let hint = match editor.state.focus.as_str() {
            "material" | "animation" => "Space Add node · F Frame · drag pins to connect",
            "vfx" => "Click a module to edit it · right-click for actions",
            "settings" => "Type to search · Ctrl+Z undoes setting changes",
            "console" | "problems" => "Click a message for details · the link goes to its source",
            "game" => {
                "Click the view to play · Esc gives the keyboard and mouse back · F5-F8 Play controls"
            }
            _ => "W E R tools · F Frame · Alt+drag Orbit · RMB+WASD Fly · Ctrl+K Commands",
        };
        let saved = self.status_note.clone().unwrap_or_else(|| {
            if self.unsaved(editor) {
                "Saving…".into()
            } else {
                "All changes saved".into()
            }
        });
        let agents = editor.state.presence.len();
        let active_agent = editor.activity.agent_active(std::time::Instant::now());
        self.bind_command("status-connection", "window.agents", Target::None);
        out.push_str(&format!("<panel id='statusbar' class='statusbar'><button id='status-connection' class='chip' label='Connection' description='This editor is the project authority; agents and the ara CLI connect to it' action='Dispatch'><panel id='connection-dot' class='status-dot' /><text id='status' class='caption' text='{}' /></button>",
            match agents {
                0 => "Connected".to_owned(),
                1 => format!("Connected · 1 agent{}", if active_agent { " editing" } else { "" }),
                n => format!("Connected · {n} agents{}", if active_agent { " editing" } else { "" }),
            }));
        let stopping = self
            .game_start
            .as_ref()
            // Once the game shows, it is playing whatever the last status read said.
            .filter(|_| editor.state.playing.is_some() && !self.game_live)
            .map(|start| start.stopping);
        if let Some(stopping) = stopping {
            self.bind_command("status-play", "window.game", Target::None);
            let text = if stopping {
                "Stopping Play…"
            } else {
                "Starting Play…"
            };
            out.push_str(&format!("<button id='status-play' class='chip' label='Play' description='Open the Game view' action='Dispatch'>{}<text id='status-play-text' class='fg' text='{text}' /></button>",icons::icon_in("status-play","loader-circle","accent")));
        } else if let Some(playing) = editor.state.playing {
            self.bind_command("status-play", "window.game", Target::None);
            out.push_str(&format!("<button id='status-play' class='chip' label='Play' description='Open the Game view' action='Dispatch'>{}<text id='status-play-text' class='{}' text='{}' /></button>",icons::icon_in("status-play",if playing{"play"}else{"pause"},if playing{"tint-play"}else{"warning"}),if playing{"fg"}else{"warning"},if playing{"Playing"}else{"Paused"}));
        }
        self.bind_command("status-entities", "window.hierarchy", Target::None);
        out.push_str(&format!("<button id='status-entities' class='chip' label='Entities' action='Dispatch'>{}<text id='status-docs' text='{} entities{}' /></button><text id='status-saved' class='caption' text='{}' style='padding-left:8px' /><panel id='status-fill' class='fill' /><text id='status-hint' class='caption' text='{}' style='padding-right:12px' />",
            icons::icon("status-entities","box"),
            self.entity_count,
            if editor.state.selection.is_empty(){String::new()}else{format!(" · {} selected",editor.state.selection.len())},
            escape(&saved),
            escape(hint)));
        // Scene quality: the active preset and where it comes from; opens the Lit menu.
        self.bindings.insert(
            ara_ui::source::stable_id("status-quality"),
            Binding::ContextMenu(Context::ViewModes, Target::None),
        );
        let state = &editor.state;
        let quality = state.quality_label(state.render_choice());
        let source = match state.render_quality {
            None => "the project's default (Project Settings > Rendering)",
            Some(ara_render_fidelity::presets::QualityChoice::Auto) => "detected from this GPU",
            Some(_) => "your override",
        };
        let description = format!("Scene quality {quality}: {source}. Change it in the Lit menu");
        out.push_str(&format!("<button id='status-quality' class='chip' label='Scene quality' description='{}' action='Dispatch'>{}<text id='status-quality-text' text='{}' /></button>",escape(&description),icons::icon("status-quality","sun-medium"),escape(&quality)));
        self.import_chip(out);
        self.bind_command("status-jobs", "window.assets", Target::None);
        // A running build leads (its stage, or cooking counts), then other jobs.
        let build = self.build.as_ref().map(|job| match job.cooking {
            Some((done, total)) if total > 0 => (
                format!("Build {} · cooking {done}/{total}", job.preset),
                done as f64 / total as f64,
            ),
            _ => (format!("Build {} · {}", job.preset, job.stage), 0.),
        });
        let count = active.len() + usize::from(build.is_some());
        let first = build.or_else(|| {
            active.first().map(|job| {
                (
                    job["name"].as_str().unwrap_or("Working").to_owned(),
                    job["progress"].as_f64().unwrap_or(0.).clamp(0., 1.),
                )
            })
        });
        match first {
            Some((name, progress)) => {
                let more = if count > 1 { format!(" · {} more", count - 1) } else { String::new() };
                out.push_str(&format!("<button id='status-jobs' class='chip' label='Jobs' description='{count} running or queued' action='Dispatch'>{}<text id='status-jobs-text' text='{} {:0.0}%{more}' /><panel id='status-jobs-bar' class='progress'><panel id='status-jobs-fill' class='progress-fill' style='width:{:0.0}%' /></panel></button>",icons::icon_in("status-jobs","activity","accent"),escape(&name),progress*100.,progress*100.));
            }
            None => out.push_str(&format!("<button id='status-jobs' class='chip' label='Jobs' description='No imports, cooks or builds running' action='Dispatch'>{}<text id='status-jobs-text' text='Jobs idle' /></button>",icons::icon("status-jobs","circle-check"))),
        }
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
