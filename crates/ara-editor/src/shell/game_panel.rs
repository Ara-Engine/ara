//! The Game view (D19): Play inside the editor. Every rendering client's frames arrive
//! through a shared GPU surface (`app/game.rs`), letterboxed to the aspect preset.
//! Click the view to give it the keyboard and mouse; Esc (or clicking outside)
//! gives them back. Host and Dedicated modes display simultaneous client tiles.
//! Outside Play it previews the scene through its gameplay camera, the view Play
//! starts with (`app/game_preview.rs`), or says the scene has none. While Play starts
//! the preview stays, under the start's progress, until Play's first frame.
use super::commands::{Target, menus::Context};
use super::*;

/// Aspect presets: id, label and width:height (0 = fill the panel).
pub(crate) const ASPECTS: &[(&str, &str, f32)] = &[
    ("free", "Free Aspect", 0.),
    ("16x9", "16:9", 16. / 9.),
    ("16x10", "16:10", 16. / 10.),
    ("4x3", "4:3", 4. / 3.),
    ("21x9", "21:9", 21. / 9.),
    ("9x16", "9:16 Portrait", 9. / 16.),
];

impl Shell {
    pub(super) fn game_panel(&mut self, editor: &mut Editor, out: &mut String) {
        let playing = editor.state.playing.is_some();
        out.push_str("<panel id='game-toolbar' class='viewport-bar'>");
        if self.game_players.len() > 1 {
            self.choice_button(
                "game-client",
                "Controlled player",
                choices::Select {
                    destination: choices::Destination::PlayClient,
                    options: self
                        .game_players
                        .iter()
                        .map(|&player| {
                            (
                                json!(player),
                                play_controls::player_label(player, self.game_players.len()),
                            )
                        })
                        .collect(),
                    current: json!(self.game_client),
                },
                out,
            );
        }
        self.tool_icon(
            out,
            "game-options",
            "play.options",
            "settings-2",
            editor,
            self.play_controls.options,
        );
        let aspect = ASPECTS
            .iter()
            .find(|a| a.0 == self.game_aspect)
            .map_or("Free Aspect", |a| a.1);
        self.tool_menu(
            out,
            "game-aspect",
            Context::GameAspect,
            "monitor",
            aspect,
            "Aspect ratio of the Game view",
        );
        out.push_str("<panel id='game-fill' class='fill' />");
        self.tool_icon(
            out,
            "game-stats",
            "game.stats",
            "gauge",
            editor,
            self.game_stats,
        );
        out.push_str("</panel>");
        self.play_controls_markup(editor, out);
        if let Some(status) = self.game_commands_status {
            out.push_str(&format!("<text id='game-command-refresh' class='caption' style='padding:4px 10px;flex-shrink:0' text='{}' />", escape(status)));
        }
        out.push_str("<panel id='game-surface' class='game-surface'>");
        if let Some(fault) = self.game_fault.clone() {
            let text = format!("{fault} · Details and fixes are in the Console.");
            if self.game_start_failed {
                self.empty(
                    out,
                    "game-fault",
                    "circle-x",
                    "Play could not start",
                    &text,
                    &[("game-stop", "Stop", "play.stop")],
                );
            } else {
                self.empty(
                    out,
                    "game-fault",
                    "circle-x",
                    "Play stopped",
                    &text,
                    &[
                        ("game-restart", "Restart", "play.restart"),
                        ("game-stop", "Stop", "play.stop"),
                    ],
                );
            }
        } else if !self.game_players.is_empty() {
            self.game_tiles(out);
        } else if playing && self.game_preview.is_some() {
            self.preview_frame(out, "The view Play is starting with");
            self.starting_hint(out);
        } else if playing {
            self.starting_markup(out);
        } else if self.gameplay_camera(editor).is_none() {
            self.empty(
                out,
                "game-empty",
                "camera",
                "No gameplay camera",
                "Play shows the scene through its Camera with the lowest Priority. Add one, or press Play (F5) to run the scene without one.",
                &[
                    ("game-camera", "Add camera", "create.camera"),
                    ("game-play", "Play", "play.start"),
                ],
            );
        } else if self.game_preview.is_some() {
            self.preview_frame(out, "The scene through its gameplay camera, as Play starts");
            out.push_str("<panel id='game-capture' class='game-hint'><text id='game-capture-text' text='Gameplay camera · Play (F5) runs the scene here' /></panel>");
        } else {
            let in_view = editor.plays_in_game_view();
            self.empty(
                out,
                "game-empty",
                "gamepad-2",
                "Game view",
                if in_view {
                    "Press Play (F5) to run the scene here. Host and Dedicated modes show every client together."
                } else {
                    "Play opens a separate player window (Preferences > Viewport > Play in the Game view)."
                },
                &[("game-play", "Play", "play.start")],
            );
        }
        out.push_str("</panel>");
    }
}

impl Shell {
    /// The Game frame's size (logical px) in the current surface: the Game surface as
    /// last laid out, letterboxed to the aspect preset; `None` when this surface does
    /// not show it (or before layout).
    pub(crate) fn game_frame(&self) -> Option<[f32; 2]> {
        self.game_frame_in(&self.ui)
    }
    /// Where the Game view is on screen: the floating window hosting it (`None` for
    /// the main window) and its frame (logical px). Play's surface and the camera
    /// preview render at that window's scale. Call with the main window current.
    pub fn game_host(&self) -> Option<(Option<&str>, [f32; 2])> {
        if self.current.is_some() {
            return None;
        }
        if let Some(frame) = self.game_frame() {
            return Some((None, frame));
        }
        self.floats
            .iter()
            .find_map(|surface| Some((Some(surface.id.as_str()), self.game_frame_in(&surface.ui)?)))
    }
    fn game_frame_in(&self, ui: &Ui) -> Option<[f32; 2]> {
        let r = ui
            .find("game-surface")
            .map(|i| ui.nodes[i].rect)
            .filter(|r| r.w > 1. && r.h > 1.)?;
        let (columns, rows) = play_controls::grid_shape(self.game_players.len().max(1));
        let r = Box2 {
            x: r.x,
            y: r.y,
            w: ((r.w - (columns - 1) as f32 * 6.) / columns as f32).max(1.),
            h: ((r.h - (rows - 1) as f32 * 6.) / rows as f32
                - if self.game_players.is_empty() {
                    0.
                } else {
                    24.
                })
            .max(1.),
        };
        let ratio = ASPECTS
            .iter()
            .find(|a| a.0 == self.game_aspect)
            .map_or(0., |a| a.2);
        Some(if ratio <= 0. {
            [r.w, r.h]
        } else if r.w / r.h > ratio {
            [r.h * ratio, r.h]
        } else {
            [r.w, r.w / ratio]
        })
    }
    /// The scene's gameplay camera as Play starts with it (`view_camera::gameplay`:
    /// the lowest `Camera.priority`, a follow rig behind its target), resolved once
    /// per scene revision.
    pub(crate) fn gameplay_camera(&mut self, editor: &Editor) -> Option<crate::state::Camera> {
        let resolved = self.resolved(editor).ok()?;
        let key = self.resolved_key()?;
        if let Some((cached, camera)) = &self.game_camera
            && *cached == key
        {
            return *camera;
        }
        let transforms = crate::world::world_transforms(&resolved);
        let camera = crate::view_camera::gameplay(&resolved, &transforms);
        self.game_camera = Some((key, camera));
        camera
    }
}

/// A Play world still starting (a `background` start, `play.status`): its phase, the
/// phase's progress and the time since Play was pressed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StartProgress {
    pub phase: String,
    pub completed: u64,
    pub total: u64,
    /// Tenths of a second since the click (the precision shown).
    pub tenths: u64,
    /// Stop was pressed; the start is being cancelled.
    pub stopping: bool,
    /// Replacing a previous world rather than starting a new one.
    pub restarting: bool,
}
impl StartProgress {
    /// From a `play.status` world row that is starting or stopping.
    pub(crate) fn from_status(row: &Value) -> Self {
        let progress = &row["progress"];
        Self {
            phase: progress["phase"].as_str().unwrap_or("queued").to_owned(),
            completed: progress["completed"].as_u64().unwrap_or(0),
            total: progress["total"].as_u64().unwrap_or(0),
            tenths: (progress["elapsedMs"].as_f64().unwrap_or(0.) / 100.) as u64,
            stopping: row["status"] == "stopping",
            restarting: row["restart"] == true,
        }
    }
    pub(super) fn title(&self) -> &'static str {
        if self.stopping {
            "Stopping Play…"
        } else if self.restarting {
            "Restarting Play…"
        } else {
            "Starting Play…"
        }
    }
    /// What the phase is doing, for people.
    fn label(&self) -> String {
        match self.phase.as_str() {
            "queued" => "Waiting for the previous Play".into(),
            "snapshot" => "Capturing the scene".into(),
            "assets" => "Preparing assets".into(),
            "pool" => "Waiting for a player".into(),
            "device" => "Starting the renderer".into(),
            "world" => "Loading the scene".into(),
            "catalog" => "Loading scripts".into(),
            other => super::inspector_panel::sentence(other),
        }
    }
}
impl Shell {
    /// Reads the shown Play world's row from `play.status`: its start progress, a
    /// start that failed, or a fault while running (the Game view and status bar
    /// show them). The window calls it on a cadence; tests call it directly.
    pub fn read_play_status(&mut self, editor: &mut Editor) {
        let status = editor
            .action("play.status", json!({}))
            .unwrap_or(Value::Null);
        let worlds = status["worlds"].as_array().cloned().unwrap_or_default();
        let shown = worlds
            .iter()
            .find(|w| w["world"].as_str() == editor.play_world.as_deref())
            .or(worlds.last());
        let state = shown.and_then(|w| w["status"].as_str()).unwrap_or("");
        self.game_start = shown
            .filter(|_| matches!(state, "starting" | "stopping"))
            .map(StartProgress::from_status);
        self.game_start_failed = state == "failed";
        self.game_running = state == "running";
        self.game_commands_status = shown.and_then(command_refresh_hint);
        self.game_fault = shown.and_then(|w| {
            if self.game_start_failed {
                return Some(
                    w["error"]
                        .as_str()
                        .or_else(|| w["diagnostics"][0]["message"].as_str())
                        .unwrap_or("the player could not start")
                        .to_owned(),
                );
            }
            let fault = &w["fault"];
            (!fault.is_null()).then(|| {
                fault["message"]
                    .as_str()
                    .unwrap_or("the player stopped")
                    .to_owned()
            })
        });
    }
    /// The gameplay camera's preview image, letterboxed like Play's frames.
    fn preview_frame(&self, out: &mut String, description: &str) {
        let [w, h] = self.game_frame().unwrap_or([640., 360.]);
        out.push_str(&format!("<panel id='game-frame' class='game-frame' style='width:{w:.0}px;height:{h:.0}px' label='Gameplay camera' description='{description}'><image id='game-image' src='game-preview' style='width:100%;height:100%' /></panel>"));
    }
    /// Play's start over the camera preview: title, phase, a bar and count when the
    /// phase has a total, and the elapsed time, in one hint. Its ids are the starting
    /// screen's, so `patch_start_progress` updates either in place; Stop is the
    /// toolbar's.
    fn starting_hint(&self, out: &mut String) {
        let start = self.game_start.as_ref();
        let title = start.map_or("Starting Play…", StartProgress::title);
        let detail = start
            .filter(|s| !s.stopping)
            .map(StartProgress::label)
            .unwrap_or_default();
        out.push_str(&format!("<panel id='game-starting' class='game-hint' style='flex-direction:row;gap:8px'><text id='game-starting-title' text='{title}' /><text id='game-starting-phase' style='white-space:nowrap' text='{}' />",escape(&detail)));
        if let Some(start) = start {
            if start.total > 0 && !start.stopping {
                let percent = start.completed as f64 * 100. / start.total as f64;
                out.push_str(&format!("<panel id='game-starting-bar' class='progress' style='width:80px'><panel id='game-starting-fill' class='progress-fill' style='width:{percent:.0}%' /></panel><text id='game-starting-count' text='{} / {}' />",start.completed,start.total));
            }
            out.push_str(&format!(
                "<text id='game-starting-time' text='{:.1} s' />",
                start.tenths as f64 / 10.
            ));
        }
        out.push_str("</panel>");
    }
    /// The Game view while Play starts: the phase, its progress (a bar when it has a
    /// total), the elapsed time and Stop; "Starting Play…" until the first status.
    fn starting_markup(&mut self, out: &mut String) {
        let Some(start) = self.game_start.clone() else {
            self.empty(
                out,
                "game-starting",
                "gamepad-2",
                "Starting Play…",
                "The game appears here; Esc gives the keyboard back to the editor.",
                &[("game-stop", "Stop", "play.stop")],
            );
            return;
        };
        let title = start.title();
        let detail = if start.stopping { String::new() } else { start.label() };
        let seconds = format!("{:.1} s", start.tenths as f64 / 10.);
        out.push_str(&format!("<panel id='game-starting' class='empty'>{}<text id='game-starting-title' class='empty-title' text='{title}' /><text id='game-starting-phase' class='empty-hint' style='white-space:nowrap' text='{}' />",icons::icon_in("game-starting","gamepad-2","icon-lg"),escape(&detail)));
        // A phase with a total: a bar and its count on one line.
        if start.total > 0 && !start.stopping {
            let percent = start.completed as f64 * 100. / start.total as f64;
            out.push_str(&format!("<panel id='game-starting-progress' class='row' style='gap:8px;align-items:center'><panel id='game-starting-bar' class='progress' style='width:160px'><panel id='game-starting-fill' class='progress-fill' style='width:{percent:.0}%' /></panel><text id='game-starting-count' class='caption' text='{} / {}' /></panel>",start.completed,start.total));
        }
        out.push_str(&format!(
            "<text id='game-starting-time' class='caption' text='{}' />",
            escape(&seconds)
        ));
        if !start.stopping {
            self.bind_command("game-stop", "play.stop", Target::None);
            out.push_str("<panel id='game-starting-actions' class='empty-actions'><button id='game-stop' label='Stop' description='Cancel starting Play' action='Dispatch'><text id='game-stop-label' text='Stop' /></button></panel>");
        }
        out.push_str("</panel>");
    }
    /// Play's start progress moved from `before` without changing the shape of the
    /// Game view's starting screen (still starting or still stopping; a bar or not):
    /// update its phase, bar, count and time in place, as `starting_markup` would
    /// write them, instead of rebuilding the editor ten times a second while Play
    /// starts. False when the shape changed, or the screen is in a floating window,
    /// and the caller must rebuild.
    pub(crate) fn patch_start_progress(
        &mut self,
        before: Option<&StartProgress>,
    ) -> ara_ui::Result<bool> {
        let (Some(before), Some(now)) = (before, self.game_start.clone()) else {
            return Ok(false);
        };
        if self.current.is_some()
            || before.stopping != now.stopping
            || before.restarting != now.restarting
            || (before.total > 0) != (now.total > 0)
        {
            return Ok(false);
        }
        if self.ui.find("game-starting").is_none() {
            // Shown in a floating window: rebuild there. Not shown at all: the next
            // build writes the current progress.
            return Ok(!self
                .floats
                .iter()
                .any(|f| f.ui.find("game-starting").is_some()));
        }
        let detail = if now.stopping {
            String::new()
        } else {
            now.label()
        };
        let texts = [
            ("game-starting-title", now.title().to_owned()),
            ("game-starting-phase", detail),
            (
                "game-starting-count",
                format!("{} / {}", now.completed, now.total),
            ),
            (
                "game-starting-time",
                format!("{:.1} s", now.tenths as f64 / 10.),
            ),
        ];
        for (id, text) in texts {
            if let Some(i) = self.ui.find(id) {
                self.ui.set_text(i, &text)?;
            }
        }
        if now.total > 0
            && let Some(i) = self.ui.find("game-starting-fill")
        {
            let percent = now.completed as f64 * 100. / now.total as f64;
            self.ui.set_style(i, &format!("width:{percent:.0}%"))?;
        }
        self.ui.forget_part("body-game");
        Ok(true)
    }
}

/// Keep the last published C# commands usable while a fresh catalog is pending.
pub(super) fn command_refresh_hint(row: &Value) -> Option<&'static str> {
    if row["commandsRefreshing"] == true {
        Some("Refreshing C# commands…")
    } else if row["commandsStale"] == true {
        Some("C# command refresh failed; showing the last good list. See Console.")
    } else {
        None
    }
}

/// The Game view's stats line: frame rate (a dash until it is measured) and size.
pub(crate) fn game_stats_text(fps: u32, size: [u32; 2]) -> String {
    let rate = if fps == 0 {
        "—".to_owned()
    } else {
        fps.to_string()
    };
    format!("{rate} fps · {} × {}", size[0], size[1])
}
