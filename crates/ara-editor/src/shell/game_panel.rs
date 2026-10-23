//! The Game view (D19): Play inside the editor. The selected client's frames arrive
//! through a shared GPU surface (`app/game.rs`), letterboxed to the aspect preset.
//! Click the view to give it the keyboard and mouse; Esc (or clicking outside)
//! gives them back. Host + clients and Dedicated + clients show a client selector.
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
        if self.game_live {
            // Letterbox inside the surface using last frame's layout.
            let area = self
                .ui
                .find("game-surface")
                .map(|i| self.ui.nodes[i].rect)
                .filter(|r| r.w > 1. && r.h > 1.);
            let ratio = ASPECTS
                .iter()
                .find(|a| a.0 == self.game_aspect)
                .map_or(0., |a| a.2);
            let (w, h) = match area {
                Some(r) if ratio > 0. => {
                    if r.w / r.h > ratio {
                        (r.h * ratio, r.h)
                    } else {
                        (r.w, r.w / ratio)
                    }
                }
                Some(r) => (r.w, r.h),
                None => (640., 360.),
            };
            self.bind_command("game-frame", "game.capture", Target::None);
            out.push_str(&format!("<button id='game-frame' class='game-frame' style='width:{w:.0}px;height:{h:.0}px' label='Game view' description='Click to play: keyboard and mouse go to the game until Esc' action='Dispatch'><image id='game-image' src='game-view' style='width:100%;height:100%' /></button>"));
            if self.game_stats {
                out.push_str(&format!("<panel id='game-stats-overlay' class='game-stats'><text id='game-stats-text' class='mono' text='{} fps · {} × {}' /></panel>",self.game_fps,self.game_size[0],self.game_size[1]));
            }
            if self.game_captured {
                out.push_str("<panel id='game-capture' class='game-hint'><text id='game-capture-text' text='Playing · Esc releases the keyboard and mouse' /></panel>");
            } else {
                out.push_str("<panel id='game-capture' class='game-hint'><text id='game-capture-text' text='Click the view to play' /></panel>");
            }
        } else if playing {
            self.empty(out, "game-starting", "gamepad-2", "Starting Play…", "The player process is loading the scene. The game appears here; Esc gives the keyboard back to the editor.", &[("game-stop", "Stop", "play.stop")]);
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
