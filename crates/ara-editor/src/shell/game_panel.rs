//! The Game view (D19): Play inside the editor. The selected client's frames arrive
//! through a shared GPU surface (`app/game.rs`), letterboxed to the aspect preset.
//! Click the view to give it the keyboard and mouse; Esc (or clicking outside)
//! gives them back. Host + clients and Dedicated + clients show a client selector.
//! Outside Play it previews the scene through its gameplay camera, the view Play
//! starts with (`app/game_preview.rs`), or says the scene has none.
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
            out.push_str("<panel id='game-clients' class='segment'>");
            for (n, player) in self.game_players.clone().into_iter().enumerate() {
                let id = format!("game-client-{player}");
                let label = match player {
                    0 if self.game_players.len() > 1 => "Host".to_owned(),
                    0 => "Local".to_owned(),
                    p => format!("Client {p}"),
                };
                let on = self.game_client == player
                    || (n == 0 && !self.game_players.contains(&self.game_client));
                self.bind_command(&id, "game.client", Target::Text(player.to_string()));
                out.push_str(&format!("<button id='{id}' class='segment-button{}' label='{label}' description='Show and control {label}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",if on{" selected"}else{""}));
            }
            out.push_str("</panel>");
        }
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
        } else if self.game_live {
            // Letterbox inside the surface using last frame's layout.
            let [w, h] = self.game_frame().unwrap_or([640., 360.]);
            self.bind_command("game-frame", "game.capture", Target::None);
            out.push_str(&format!("<button id='game-frame' class='game-frame' style='width:{w:.0}px;height:{h:.0}px' label='Game view' description='Click to play: keyboard and mouse go to the game until Esc' action='Dispatch'><image id='game-image' src='game-view' style='width:100%;height:100%' /></button>"));
            if self.game_stats {
                out.push_str(&format!("<panel id='game-stats-overlay' class='game-stats'><text id='game-stats-text' class='mono' text='{}' /></panel>",game_stats_text(self.game_fps,self.game_size)));
            }
            if self.game_captured {
                out.push_str("<panel id='game-capture' class='game-hint'><text id='game-capture-text' text='Playing · Esc releases the keyboard and mouse' /></panel>");
            } else {
                out.push_str("<panel id='game-capture' class='game-hint'><text id='game-capture-text' text='Click the view to play' /></panel>");
            }
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
            let [w, h] = self.game_frame().unwrap_or([640., 360.]);
            out.push_str(&format!("<panel id='game-frame' class='game-frame' style='width:{w:.0}px;height:{h:.0}px' label='Gameplay camera' description='The scene through its gameplay camera, as Play starts'><image id='game-image' src='game-preview' style='width:100%;height:100%' /></panel>"));
            out.push_str("<panel id='game-capture' class='game-hint'><text id='game-capture-text' text='Gameplay camera · Play (F5) runs the scene here' /></panel>");
        } else {
            let in_view = editor
                .state
                .settings
                .get("play.inGameView")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            self.empty(
                out,
                "game-empty",
                "gamepad-2",
                "Game view",
                if in_view {
                    "Press Play (F5) to run the scene here. Host and Dedicated modes add a client selector."
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
        let (title, detail) = if start.stopping {
            ("Stopping Play…", String::new())
        } else {
            ("Starting Play…", start.label())
        };
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
