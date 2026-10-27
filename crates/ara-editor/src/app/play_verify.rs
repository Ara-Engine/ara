//! Focused Play stories through native buttons, selects, fields and function keys.
//! Setup authoring uses typed commands; the route shares the hidden behaviour fixture.
use super::*;
use std::{collections::BTreeSet, path::Path};
const LIGHT: &str = "e_playprooflight";
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Multi,
    Runtime,
}
const ACTS: &[Act] = &[Act::Multi, Act::Runtime];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::Multi => "multiplayer-views-input",
            Self::Runtime => "runtime-patch-keep-undo",
        }
    }
    fn finish(self) -> u32 {
        if self == Self::Multi { 18 } else { 28 }
    }
}
pub(super) struct PlayCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    waiting: bool,
    failed_frame: bool,
    began: Instant,
    before: u64,
    patch_tick: u64,
    trace: Vec<Value>,
    results: Vec<Value>,
}
fn key(events: &mut Vec<(u32, Event)>, code: i32, pressed: bool) {
    events.push((
        0,
        Event::Key {
            scancode: code,
            pressed,
            repeat: false,
        },
    ));
}
fn tap(events: &mut Vec<(u32, Event)>, code: i32) {
    key(events, code, true);
    key(events, code, false);
}
fn control(shell: &Shell, id: &str) -> Option<[f32; 2]> {
    let index = shell.shown_node(id)?;
    let rect = shell.ui.nodes[index].rect;
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    if !shell.ui.nodes[index].clip.contains(point[0], point[1]) {
        return None;
    }
    Some(point)
}
fn click(
    shell: &Shell,
    id: &str,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let point = control(shell, id).ok_or_else(|| format!("Play control {id} is not reachable"))?;
    events.push((
        0,
        Event::MouseMotion {
            position: point.map(|value| value / scale),
            delta: [0.; 2],
        },
    ));
    for pressed in [true, false] {
        events.push((0, Event::MouseButton { button: 1, pressed }));
    }
    Ok(())
}
fn select(
    shell: &Shell,
    id: &str,
    index: usize,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    click(shell, id, scale, events)?;
    tap(events, 74);
    for _ in 0..index {
        tap(events, 81);
    }
    tap(events, 40);
    Ok(())
}
fn replace(
    shell: &Shell,
    id: &str,
    text: &str,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    click(shell, id, scale, events)?;
    key(events, 224, true);
    tap(events, 4);
    key(events, 224, false);
    events.push((0, Event::TextInput { text: text.into() }));
    tap(events, 40);
    Ok(())
}
fn intensity(editor: &Editor) -> Option<f64> {
    editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)?
        .record(LIGHT)?
        .get("Light")?
        .get("intensity")?
        .as_f64()
}
impl PlayCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Play verification requires the isolated behaviour fixture".into());
        }
        std::fs::create_dir_all(output)?;
        editor.apply(vec![
            json!({"cmd":"entity.create","args":{"id":LIGHT,"name":"Play proof light","components":{"Transform":{"position":[0,3,0]},"Light":{"intensity":1000}}}}),
            json!({"cmd":"entity.create","args":{"id":"e_playproofcamera","name":"Play proof camera","components":{"Transform":{"position":[0,2,10]},"Camera":{"priority":-100}}}}),
        ],"Create native Play proof fixture",None)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waiting: false,
            failed_frame: false,
            began: Instant::now(),
            before: 0,
            patch_tick: 0,
            trace: Vec::new(),
            results: Vec::new(),
        })
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|result| result["ok"] != true)
            .filter_map(|result| result["journey"].as_str().map(str::to_owned))
            .collect()
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "matches the existing hidden native verification seam"
    )]
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        _window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        if let Err(error) = self.input(act, editor, shell, scale, events) {
            events.clear();
            events.push((0, Event::FocusLost));
            key(events, 225, true);
            tap(events, 62);
            key(events, 225, false);
            self.results.push(json!({"journey":act.label(),"ok":false,"error":error.to_string(),"trace":self.trace}));
            self.advance();
            self.failed_frame = true;
            self.write()?;
        }
        Ok(())
    }
    fn wait(&mut self) -> ara_ui::Result<()> {
        if self.began.elapsed() > Duration::from_secs(180) {
            return Err("Native Play story exceeded its bounded startup/action wait".into());
        }
        self.waiting = true;
        Ok(())
    }
    fn setup(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        if editor.play_world.is_some() {
            editor.action("play.stop", json!({}))?;
            editor.action("editor.view", json!({"play":"stopped"}))?;
        }
        shell.leave();
        shell.desktop_pointer = None;
        shell.menu = None;
        shell.dialog = None;
        shell.play_controls.options = false;
        editor.action("editor.layout.reset", json!({}))?;
        editor.action("editor.layout.show", json!({"panel":"game"}))?;
        editor.action("editor.search", json!({"panel":"hierarchy","query":""}))?;
        editor.action("editor.select", json!({"entities":[LIGHT]}))?;
        for (setting, value) in [
            ("play.mode", json!("local")),
            ("play.clients", json!(1)),
            ("play.inGameView", json!(true)),
            ("play.startPaused", json!(false)),
            ("render.quality", json!("low")),
        ] {
            editor.action("editor.settings.set", json!({"key":setting,"value":value}))?;
        }
        editor.apply(vec![json!({"cmd":"property.set","args":{"entity":LIGHT,"path":"Light.intensity","value":1000}})],"Reset Play proof light",None)?;
        shell.rebuild(editor)?;
        shell.input.blur(&mut shell.ui);
        self.before = editor.host.store().revision();
        Ok(())
    }
    fn input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !scale.is_finite() || scale <= 0. {
            return Err("Play proof native input scale is invalid".into());
        }
        if act == Act::Runtime && self.age > 9 && shell.play_runtime_report()["pending"] == true {
            return self.wait();
        }
        let target = match (act, self.age) {
            (_, 2) => Some("game-options"),
            (Act::Multi, 4) => Some("play-mode"),
            (Act::Multi, 5) => Some("play-client-count"),
            (Act::Multi, 6) => Some("play-network-1"),
            (Act::Runtime, 4) => Some("play-start-paused"),
            (Act::Runtime, 9 | 20) => Some("play-inspect"),
            (Act::Runtime, 13) => Some("play-keep"),
            _ => None,
        };
        if let Some(target) = target
            && control(shell, target).is_none()
        {
            if let (Some(node), Some(scroll)) =
                (shell.shown_node(target), shell.shown_node("play-options"))
            {
                let rect = shell.ui.nodes[scroll].rect;
                let direction = if shell.ui.nodes[node].rect.y < rect.y {
                    4.
                } else {
                    -4.
                };
                events.push((
                    0,
                    Event::MouseMotion {
                        position: [
                            (rect.x + rect.w * 0.5) / scale,
                            (rect.y + rect.h * 0.5) / scale,
                        ],
                        delta: [0.; 2],
                    },
                ));
                events.push((0, Event::Wheel([0., direction])));
            }
            return self.wait();
        }
        match self.age {
            0 => {
                self.began = Instant::now();
                events.push((0, Event::FocusLost));
                self.setup(editor, shell)?;
            }
            2 => click(shell, "game-options", scale, events)?,
            4 if act == Act::Multi => select(shell, "play-mode", 1, scale, events)?,
            4 => click(shell, "play-start-paused", scale, events)?,
            5 if act == Act::Multi => replace(shell, "play-client-count", "2", scale, events)?,
            6 if act == Act::Multi => select(shell, "play-network-1", 2, scale, events)?,
            7 => tap(events, 62),
            8 => {
                let expected = if act == Act::Multi { 3 } else { 1 };
                if !shell.game_running
                    || shell.play_controls.players.len() != expected
                    || !shell
                        .play_controls
                        .players
                        .values()
                        .all(|player| player.live)
                {
                    return self.wait();
                }
            }
            9 if act == Act::Multi => click(shell, "game-frame-2", scale, events)?,
            9 => click(shell, "play-inspect", scale, events)?,
            10 if act == Act::Runtime => {
                let id = format!("runtime-{}", ara_ui::source::stable_id("Light.intensity#"));
                if control(shell, &id).is_none() {
                    let index = shell
                        .shown_node("play-options")
                        .ok_or("Play options scroll is missing")?;
                    let rect = shell.ui.nodes[index].rect;
                    events.push((
                        0,
                        Event::MouseMotion {
                            position: [
                                (rect.x + rect.w * 0.5) / scale,
                                (rect.y + rect.h * 0.5) / scale,
                            ],
                            delta: [0.; 2],
                        },
                    ));
                    events.push((0, Event::Wheel([0., -4.])));
                    return self.wait();
                }
                replace(shell, &id, "2500", scale, events)?;
            }
            11 if act == Act::Multi => tap(events, 41),
            11 => {
                let report = shell.play_runtime_report();
                if report["inspection"]["components"]["Light"]["intensity"].as_f64() != Some(2500.)
                {
                    return self.wait();
                }
                self.patch_tick = report["inspection"]["tick"].as_u64().unwrap_or(0);
            }
            13 if act == Act::Multi => {
                key(events, 225, true);
                tap(events, 62);
                key(events, 225, false);
            }
            13 => click(shell, "play-keep", scale, events)?,
            15 if act == Act::Runtime => {
                key(events, 224, true);
                tap(events, 29);
                key(events, 224, false);
            }
            17 if act == Act::Runtime => {
                key(events, 224, true);
                tap(events, 28);
                key(events, 224, false);
            }
            19 if act == Act::Runtime => tap(events, 64),
            20 if act == Act::Runtime => click(shell, "play-inspect", scale, events)?,
            21 if act == Act::Runtime => tap(events, 62),
            22 if act == Act::Runtime => tap(events, 63),
            23 if act == Act::Runtime => {
                key(events, 225, true);
                tap(events, 62);
                key(events, 225, false);
            }
            24 if act == Act::Runtime => {
                if editor.play_world.is_some() || editor.state.playing.is_some() {
                    return self.wait();
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if std::mem::take(&mut self.failed_frame) || self.finished() || self.waiting {
            return Ok(());
        }
        let act = ACTS[self.index];
        let stage = match (act, self.age) {
            (_, 3) => Some("before"),
            (Act::Multi, 10) => Some("preview"),
            (Act::Multi, 12) => Some("released"),
            (Act::Runtime, 12) => Some("preview"),
            (Act::Runtime, 14) => Some("kept"),
            (Act::Runtime, 16) => Some("undo"),
            (Act::Runtime, 18) => Some("redo"),
            (Act::Runtime, 20) => Some("stepped"),
            (Act::Runtime, 21) => Some("resumed"),
            (Act::Runtime, 22) => Some("paused"),
            (_, age) if age == act.finish() => Some("after"),
            _ => None,
        };
        if let Some(stage) = stage {
            gpu.select_target(0);
            let prefix = format!("play-{:02}-{}-{stage}", self.index + 1, act.label());
            gpu.pixels()?
                .save(self.output.join(format!("{prefix}.png")))?;
            std::fs::write(
                self.output.join(format!("{prefix}.layout.json")),
                ara_json::to_canonical_string(&shell.layout_report()),
            )?;
            let frames:Vec<_>=shell.game_players.iter().filter_map(|player|{let node=shell.shown_node(&format!("game-frame-{player}"))?;let r=shell.ui.nodes[node].rect;Some(json!({"player":player,"rect":[r.x,r.y,r.w,r.h],"texture":game::player_texture(*player)}))}).collect();
            self.trace.push(json!({"stage":stage,"revision":editor.host.store().revision(),"authoringIntensity":intensity(editor),"playing":editor.state.playing,"world":editor.play_world,"runtime":shell.play_runtime_report(),"frames":frames,"settings":{"mode":editor.state.settings.get("play.mode"),"clients":editor.state.settings.get("play.clients"),"profiles":editor.state.settings.get("play.networkProfiles")}}));
        }
        if self.age == act.finish() {
            let trace = |stage: &str| self.trace.iter().find(|trace| trace["stage"] == stage);
            let ok = if act == Act::Multi {
                trace("preview").is_some_and(|row| {
                    row["runtime"]["controlledPlayer"] == 2
                        && row["runtime"]["captured"] == true
                        && row["frames"].as_array().is_some_and(|frames| {
                            frames.len() == 3
                                && frames
                                    .iter()
                                    .filter_map(|frame| frame["texture"].as_u64())
                                    .collect::<BTreeSet<_>>()
                                    .len()
                                    == 3
                        })
                        && row["settings"]["mode"] == "host"
                        && row["settings"]["clients"].as_f64() == Some(2.)
                        && row["settings"]["profiles"]["1"] == "internet"
                }) && trace("released").is_some_and(|row| row["runtime"]["captured"] == false)
                    && editor.host.store().revision() == self.before
                    && editor.play_world.is_none()
            } else {
                trace("preview").is_some_and(|row| {
                    row["authoringIntensity"].as_f64() == Some(1000.)
                        && row["runtime"]["inspection"]["components"]["Light"]["intensity"].as_f64()
                            == Some(2500.)
                        && row["revision"] == self.before
                }) && trace("kept").is_some_and(|row| {
                    row["authoringIntensity"].as_f64() == Some(2500.)
                        && row["revision"] == self.before + 1
                }) && trace("undo")
                    .is_some_and(|row| row["authoringIntensity"].as_f64() == Some(1000.))
                    && trace("redo")
                        .is_some_and(|row| row["authoringIntensity"].as_f64() == Some(2500.))
                    && trace("stepped").is_some_and(|row| {
                        row["runtime"]["inspection"]["tick"].as_u64() == Some(self.patch_tick + 1)
                    })
                    && trace("resumed").is_some_and(|row| row["playing"] == true)
                    && trace("paused").is_some_and(|row| row["playing"] == false)
                    && editor.play_world.is_none()
                    && editor.state.playing.is_none()
            };
            self.results
                .push(json!({"journey":act.label(),"ok":ok,"trace":self.trace}));
            self.advance();
            self.write()?;
        } else {
            self.age += 1;
        }
        Ok(())
    }
    fn advance(&mut self) {
        self.index += 1;
        self.age = 0;
        self.trace.clear();
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("play-behaviours.json"),
            ara_json::to_canonical_string(
                &json!({"journeys":self.results,"completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len()}),
            ),
        )?;
        Ok(())
    }
}
