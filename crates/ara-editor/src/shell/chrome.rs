//! Compact status row; the custom caption is in titlebar.rs.
use super::commands::{Target, menus::Context};
use super::*;
impl Shell {
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
            _ => "Q W E R T tools · F Frame · Alt+drag Orbit · RMB+WASD Fly · Ctrl+K Commands",
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
        let starting = self
            .game_start
            .as_ref()
            // Once the game shows, it is playing whatever the last status read said.
            .filter(|_| editor.state.playing.is_some() && !self.game_live)
            .map(game_panel::StartProgress::title);
        if let Some(text) = starting {
            self.bind_command("status-play", "window.game", Target::None);
            out.push_str(&format!("<button id='status-play' class='chip' label='Play' description='Open the Game view' action='Dispatch'>{}<text id='status-play-text' class='fg' text='{text}' /></button>",icons::icon_in("status-play","loader-circle","accent")));
        } else if let Some(playing) = editor.state.playing {
            self.bind_command("status-play", "window.game", Target::None);
            out.push_str(&format!("<button id='status-play' class='chip' label='Play' description='Open the Game view' action='Dispatch'>{}<text id='status-play-text' class='{}' text='{}' /></button>",icons::icon_in("status-play",if playing{"play"}else{"pause"},if playing{"tint-play"}else{"warning"}),if playing{"fg"}else{"warning"},if playing{"Playing"}else{"Paused"}));
        }
        self.bind_command("status-entities", "window.hierarchy", Target::None);
        out.push_str(&format!("<button id='status-entities' class='chip' label='Entities' action='Dispatch'>{}<text id='status-docs' text='{} entities{}' /></button><text id='status-saved' class='caption' text='{}' style='padding-left:8px' /><panel id='status-fill' class='fill' /><text id='status-hint' class='caption' text='{}' style='flex-shrink:1;min-width:0px;overflow:hidden;padding-right:12px' />",
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
        self.import_chip(editor, out);
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
        self.status_tools(editor, out);
        self.bind_command("status-frame", "window.profiler", Target::None);
        out.push_str(&format!("<button id='status-frame' class='chip' label='Frame time' description='Open the Profiler' action='Dispatch'>{}<text id='status-timing' text='144 FPS · 6.94 ms' /></button></panel>",icons::icon("status-frame","gauge")));
    }
    /// Compact command search and project-only diagnostics live beside status.
    fn status_tools(&mut self, editor: &mut Editor, out: &mut String) {
        let mut count = |severity: &str, codes: Option<&[&str]>| {
            let mut query = json!({"severity":severity,"group":false,"limit":1});
            if let Some(codes) = codes {
                query["codes"] = json!(codes);
            }
            editor
                .action("logs", query)
                .ok()
                .and_then(|logs| logs["total"].as_u64())
                .unwrap_or(0)
        };
        let machine: &[&str] = &["ARA_GPU_ENVIRONMENT"];
        let errors = count("error", None).saturating_sub(count("error", Some(machine)));
        let problems = count("warning", None).saturating_sub(count("warning", Some(machine)));
        if problems > 0 {
            self.bind_command("chip-problems", "window.problems", Target::None);
            let tint = if errors > 0 { "destructive" } else { "warning" };
            let warnings = problems.saturating_sub(errors);
            out.push_str(&format!("<button id='chip-problems' class='chip' label='Problems' description='{errors} errors, {warnings} warnings · open Problems' action='Dispatch'>{}<text id='chip-problems-text' class='{tint}' text='{problems}' /></button>",icons::icon_in("chip-problems","triangle-alert",tint)));
        }
        self.bind_command("search-box", "edit.palette", Target::None);
        out.push_str(&format!("<button id='search-box' class='ghost icon-button' label='Search or run a command' description='Search commands, entities and assets · Ctrl+K' action='Dispatch'>{}</button>",icons::icon("search-box","search")));
        self.bind_command("chip-settings", "edit.preferences", Target::None);
        out.push_str(&format!("<button id='chip-settings' class='ghost icon-button' label='Preferences' description='Preferences · Ctrl+,' action='Dispatch'>{}</button>",icons::icon("chip-settings","settings-2")));
    }
    pub(super) fn unsaved(&self, editor: &Editor) -> bool {
        editor.host.unsaved()
    }
}
