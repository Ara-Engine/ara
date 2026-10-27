//! Human Play adapters. RPC calls occur only for an explicit Inspect/Apply/Keep,
//! never during panel painting or the display frame loop.
use super::*;
use ara_schema::ValueType;
mod fields;
mod jobs;
mod network;
mod runtime;
pub(crate) use network::conditions;

pub(crate) fn player_label(player: usize, count: usize) -> String {
    match player {
        0 if count > 1 => "Host".into(),
        0 => "Local".into(),
        player => format!("Client {player}"),
    }
}
pub(crate) fn grid_shape(count: usize) -> (usize, usize) {
    let count = count.max(1);
    let columns = (count as f64).sqrt().ceil() as usize;
    (columns, count.div_ceil(columns))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlayerInfo {
    pub live: bool,
    pub size: [u32; 2],
    pub fps: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FieldTarget {
    pub path: String,
    pub pointer: String,
    pub ty: ValueType,
    pub unit: Option<String>,
}
impl FieldTarget {
    fn key(&self) -> String {
        format!("{}#{}", self.path, self.pointer)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    Field(FieldTarget),
    Set(FieldTarget, Value),
    Expand(String),
    Page(String, i32),
}
struct Draft {
    target: FieldTarget,
    input: Value,
}
struct Runtime {
    world: String,
    player: usize,
    entity: String,
    tick: u64,
    components: Value,
    local: bool,
    drafts: BTreeMap<String, Draft>,
}
/// Transient presentation state; authored values are changed only by play.keep's typed batch.
#[derive(Default)]
pub(crate) struct State {
    pub options: bool,
    pub players: BTreeMap<usize, PlayerInfo>,
    /// Incremented only when the player tile set or readiness changes.
    pub revision: u64,
    runtime: Option<Runtime>,
    expanded: std::collections::BTreeSet<String>,
    pages: BTreeMap<String, usize>,
    message: Option<String>,
    pending: Option<jobs::Pending>,
    pub simulate: Option<bool>,
    spectators: std::collections::BTreeSet<usize>,
}
impl Shell {
    pub(crate) fn toggle_play_options(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.play_controls.options = !self.play_controls.options;
        self.rebuild(editor)
    }
    /// Changing the controlled player releases every held input in the previous one.
    pub(crate) fn focus_play_client(
        &mut self,
        editor: &mut Editor,
        player: usize,
        capture: bool,
    ) -> ara_ui::Result<()> {
        if !self.game_players.contains(&player) {
            return Err("This Play client no longer has a view".into());
        }
        if self
            .play_controls
            .runtime
            .as_ref()
            .is_some_and(|runtime| !runtime.drafts.is_empty() && runtime.player != player)
        {
            return Err("Apply or discard the pending runtime edits before changing client".into());
        }
        if (self.game_client != player || !capture)
            && let (Some(world), Some(surfaces)) = (&editor.play_world, &editor.play_surfaces)
        {
            surfaces.device(
                world,
                self.game_client,
                vec![ara_play_host::embed::DeviceEvent::FocusLost],
            );
        }
        self.game_client = player;
        self.game_captured = capture;
        if self
            .play_controls
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.player != player)
        {
            self.play_controls.runtime = None;
        }
        self.rebuild(editor)
    }
    pub(crate) fn dispatch_play(
        &mut self,
        editor: &mut Editor,
        action: &Action,
        value: Value,
    ) -> ara_ui::Result<()> {
        if self.play_controls.pending.is_some() {
            return Err("Wait for the current runtime operation to complete".into());
        }
        match action {
            Action::Field(target) => {
                let runtime = self
                    .play_controls
                    .runtime
                    .as_mut()
                    .ok_or("Inspect the running entity before editing it")?;
                runtime.drafts.insert(
                    target.key(),
                    Draft {
                        target: target.clone(),
                        input: value,
                    },
                );
            }
            Action::Set(target, value) => {
                let runtime = self
                    .play_controls
                    .runtime
                    .as_mut()
                    .ok_or("Inspect the running entity before editing it")?;
                runtime.drafts.insert(
                    target.key(),
                    Draft {
                        target: target.clone(),
                        input: value.clone(),
                    },
                );
                self.commit_play_fields(editor)?;
                self.rebuild(editor)?;
            }
            Action::Expand(key) => {
                if !self.play_controls.expanded.remove(key) {
                    self.play_controls.expanded.insert(key.clone());
                }
                self.rebuild(editor)?;
            }
            Action::Page(key, delta) => {
                let page = self.play_controls.pages.entry(key.clone()).or_default();
                *page = page.saturating_add_signed(*delta as isize);
                self.rebuild(editor)?;
            }
        }
        Ok(())
    }
    pub(crate) fn choose_play_field(
        &mut self,
        editor: &mut Editor,
        target: &FieldTarget,
        value: Value,
    ) -> ara_ui::Result<()> {
        self.dispatch_play(editor, &Action::Field(target.clone()), value)?;
        self.commit_play_fields(editor)?;
        self.rebuild(editor)
    }
    pub(crate) fn discard_play_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.play_controls.pending.is_some() {
            return Err("Wait for the current runtime operation to complete".into());
        }
        if let Some(runtime) = &mut self.play_controls.runtime {
            runtime.drafts.clear();
        }
        self.play_controls.message = None;
        self.rebuild(editor)
    }
    /// The explicit inspection snapshot and live tile metadata, used by native acceptance evidence.
    pub(crate) fn play_runtime_report(&self) -> Value {
        json!({"controlledPlayer":self.game_client,"captured":self.game_captured,"pending":self.play_controls.pending.is_some(),"players":self.play_controls.players.iter().map(|(player,info)|json!({"player":player,"live":info.live,"size":info.size,"fps":info.fps})).collect::<Vec<_>>(),"inspection":self.play_controls.runtime.as_ref().map(|runtime|json!({"world":runtime.world,"player":runtime.player,"entity":runtime.entity,"tick":runtime.tick,"components":runtime.components,"drafts":runtime.drafts.len(),"editable":runtime.local})),"message":self.play_controls.message})
    }
    /// Each visible tile owns its own capture rectangle; changing focus never swaps images.
    pub(crate) fn game_input_rect(&self) -> Option<Box2> {
        self.shown_node(&format!("game-frame-{}", self.game_client))
            .map(|index| self.ui.nodes[index].rect)
    }
    pub(super) fn game_tiles(&mut self, out: &mut String) {
        let count = self.game_players.len();
        let (columns, rows) = grid_shape(count);
        out.push_str("<panel id='game-grid' style='width:100%;height:100%;flex-direction:column;gap:6px;min-height:0px'>");
        for row in 0..rows {
            out.push_str(&format!("<panel id='game-grid-row-{row}' style='flex-direction:row;flex-grow:1;flex-basis:0px;min-height:0px;gap:6px'>"));
            for column in 0..columns {
                let Some(&player) = self.game_players.get(row * columns + column) else {
                    out.push_str(&format!("<panel id='game-grid-empty-{row}-{column}' style='flex-grow:1;flex-basis:0px;min-width:0px;min-height:0px' />"));
                    continue;
                };
                let label = player_label(player, count);
                let selected = player == self.game_client;
                out.push_str(&format!("<panel id='game-tile-{player}' style='flex-direction:column;flex-grow:1;flex-basis:0px;min-width:0px;min-height:0px;align-items:center;overflow:hidden'><panel id='game-tile-bar-{player}' class='row' style='height:24px;min-height:24px;width:100%;gap:6px'>"));
                self.bind_command(
                    &format!("game-focus-{player}"),
                    "game.client",
                    commands::Target::Text(player.to_string()),
                );
                out.push_str(&format!("<button id='game-focus-{player}' class='ghost{}' style='min-width:0px;flex-shrink:1' label='Focus {label}' description='{}' action='Dispatch'><text id='game-focus-label-{player}' text='{label}{}' /></button><text id='game-stats-text-{player}' class='caption mono' text='{}' /></panel>",if selected{" selected"}else{""},if selected && self.game_captured {"Keyboard and mouse captured · Esc releases"}else{"Focus this player"},if selected && self.game_captured {" · captured · Esc releases"}else if selected{" · focused"}else{""},if self.game_stats {self.play_controls.players.get(&player).map(|info|game_stats_text(info.fps,info.size)).unwrap_or_default()}else{String::new()}));
                out.push_str(&format!("<panel id='game-client-surface-{player}' style='width:100%;flex-grow:1;flex-basis:0px;min-height:0px;align-items:center;justify-content:center'>"));
                if self
                    .play_controls
                    .players
                    .get(&player)
                    .is_some_and(|info| info.live)
                {
                    self.bind_command(
                        &format!("game-frame-{player}"),
                        "game.capture_client",
                        commands::Target::Text(player.to_string()),
                    );
                    out.push_str(&format!("<button id='game-frame-{player}' class='game-frame' style='width:100%;height:100%;min-width:0px;min-height:0px' label='{label} Game view' description='Click to control {label}; Esc releases controls' action='Dispatch'><image id='game-image-{player}' src='game-view-{player}' style='width:100%;height:100%' /></button>"));
                } else {
                    out.push_str(&format!("<panel id='game-frame-{player}' class='game-frame' style='width:100%;height:100%;min-width:0px;min-height:0px;align-items:center;justify-content:center'><text id='game-wait-{player}' class='caption' text='Waiting for {label} frame…' /></panel>"));
                }
                out.push_str("</panel></panel>");
            }
            out.push_str("</panel>");
        }
        out.push_str("</panel>");
    }
    /// Fit aspect presets after the current flex layout. The container, including
    /// options/docking changes, is measured from this frame rather than the previous one.
    pub(crate) fn fit_game_frames(&mut self) -> ara_ui::Result<bool> {
        let ratio = super::game_panel::ASPECTS
            .iter()
            .find(|aspect| aspect.0 == self.game_aspect)
            .map_or(0., |aspect| aspect.2);
        let mut changed = false;
        for player in &self.game_players {
            let Some(area) = self
                .shown_node(&format!("game-client-surface-{player}"))
                .map(|node| self.ui.nodes[node].rect)
            else {
                continue;
            };
            let Some(frame) = self.shown_node(&format!("game-frame-{player}")) else {
                continue;
            };
            if ![area.w, area.h]
                .into_iter()
                .all(|value| value.is_finite() && value > 0.)
            {
                continue;
            }
            let size = if ratio <= 0. {
                [area.w, area.h]
            } else if area.w / area.h > ratio {
                [area.h * ratio, area.h]
            } else {
                [area.w, area.w / ratio]
            };
            let current = self.ui.nodes[frame].rect;
            if (current.w - size[0]).abs() > 0.25 || (current.h - size[1]).abs() > 0.25 {
                self.ui
                    .set_style(frame, &format!("width:{}px;height:{}px", size[0], size[1]))?;
                changed = true;
            }
        }
        if changed {
            self.ui.frame(&mut self.text)?;
        }
        Ok(changed)
    }
    pub(super) fn play_controls_markup(&mut self, editor: &Editor, out: &mut String) {
        if !self.play_controls.options {
            return;
        }
        out.push_str("<scroll id='play-options' style='max-height:260px;flex-shrink:0;min-height:0px'><panel id='play-options-body' style='flex-direction:column;padding:8px;gap:6px'>");
        out.push_str("<panel id='play-mode-row' class='row' style='gap:8px'><text id='play-mode-label' class='caption' text='Play mode' />");
        self.choice_button(
            "play-mode",
            "Play mode",
            choices::Select {
                destination: choices::Destination::Setting("play.mode".into()),
                options: vec![
                    (json!("local"), "Local".into()),
                    (json!("host"), "Host + clients".into()),
                    (json!("dedicated"), "Dedicated + clients".into()),
                ],
                current: editor
                    .state
                    .settings
                    .get("play.mode")
                    .cloned()
                    .unwrap_or_else(|| json!("local")),
            },
            out,
        );
        let mode = editor
            .state
            .settings
            .get("play.mode")
            .and_then(Value::as_str)
            .unwrap_or("local");
        out.push_str("</panel><panel id='play-process-row' class='row' style='gap:8px'><text id='play-process-label' class='caption' text='Processes' />");
        self.choice_button(
            "play-process-mode",
            "Process mode",
            choices::Select {
                destination: choices::Destination::Setting("play.processMode".into()),
                options: vec![
                    (json!("separate"), "Separate process per player".into()),
                    (json!("shared"), "Shared process, independent worlds".into()),
                ],
                current: editor
                    .state
                    .settings
                    .get("play.processMode")
                    .cloned()
                    .unwrap_or_else(|| json!("separate")),
            },
            out,
        );
        out.push_str("</panel><panel id='play-run-options' class='row' style='gap:8px'>");
        if mode != "local" {
            self.bindings.insert(
                ara_ui::source::stable_id("play-client-count"),
                Binding::Setting("play.clients".into()),
            );
            let clients = editor
                .state
                .settings
                .get("play.clients")
                .and_then(Value::as_f64)
                .unwrap_or(1.);
            out.push_str(&format!("<text id='play-clients-label' class='caption' text='Clients' /><field id='play-client-count' class='number' style='width:76px' min='0' max='16' step='1' text='{clients:.0}' label='Client count' description='Additional client processes. Type a count or drag the grip.' action='Dispatch' />"));
        }
        self.bindings.insert(
            ara_ui::source::stable_id("play-start-paused"),
            Binding::Setting("play.startPaused".into()),
        );
        out.push_str(&format!("<toggle id='play-start-paused' checked='{}' label='Start paused' action='Dispatch' /></panel>",editor.state.settings.get("play.startPaused").and_then(Value::as_bool).unwrap_or(false)));
        if mode != "local" {
            self.play_network_markup(editor, out);
        }
        if editor.state.playing.is_some() {
            out.push_str("<panel id='play-runtime-toolbar' style='flex-direction:column;gap:4px'><panel id='play-runtime-edit-tools' class='row' style='gap:6px'>");
            for (index, (id, title, command)) in [
                ("play-inspect", "Inspect selected", "play.inspect"),
                ("play-apply", "Apply edits", "play.runtime.patch"),
                ("play-discard", "Discard edits", "play.runtime.discard"),
                ("play-keep", "Keep selected", "play.keep"),
                ("play-keep-driven", "Keep transforms", "play.keep_driven"),
                ("play-simulate", "Simulate", "play.simulate"),
                ("play-eject", "Eject", "play.eject"),
                ("play-possess", "Possess selected", "play.possess"),
            ]
            .into_iter()
            .enumerate()
            {
                if index == 3 {
                    out.push_str(
                        "</panel><panel id='play-runtime-keep-tools' class='row' style='gap:6px'>",
                    );
                }
                self.bind_command(id, command, commands::Target::None);
                out.push_str(&format!("<button id='{id}' class='ghost' label='{title}' description='{title}' action='Dispatch'><text id='{id}-label' text='{title}' /></button>"));
            }
            out.push_str("</panel></panel>");
            self.play_runtime_markup(editor, out);
        }
        if let Some(message) = &self.play_controls.message {
            out.push_str(&format!("<text id='play-runtime-message' class='caption' style='white-space:normal' text='{}' />",escape(message)));
        }
        out.push_str("</panel></scroll>");
    }
}
