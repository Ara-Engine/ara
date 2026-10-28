//! Focused Play stories through native buttons, selects, fields and function keys.
//! Setup authoring uses typed commands; the route shares the hidden behaviour fixture.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
mod configuration;
mod waits;
const LIGHT: &str = "e_playprooflight";
const ACTOR: &str = "e_playproofactor";
const CAMERA: &str = "e_playproofcamera";
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Multi,
    MultiShared,
    Control,
    Runtime,
}
const ACTS: &[Act] = &[Act::MultiShared, Act::Multi, Act::Control, Act::Runtime];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::Multi => "separate-process-views-input",
            Self::MultiShared => "shared-process-views-input",
            Self::Control => "simulate-spectator-possess-eject",
            Self::Runtime => "runtime-patch-keep-undo",
        }
    }
    fn finish(self) -> u32 {
        match self {
            Self::Multi | Self::MultiShared => 24,
            Self::Control => 36,
            Self::Runtime => 34,
        }
    }
    fn multi(self) -> bool {
        matches!(self, Self::Multi | Self::MultiShared)
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
    initial: u64,
    scene_before: Value,
    configuration: Value,
    patch_tick: u64,
    step_began: Instant,
    probe: Option<(String, Instant)>,
    proofs: BTreeMap<String, Value>,
    trace: Vec<Value>,
    results: Vec<Value>,
    waits: waits::Samples,
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
fn light_range(editor: &Editor) -> Option<f64> {
    let light = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)?
        .record(LIGHT)?
        .get("Light")?;
    Some(
        light
            .get("range")
            .and_then(Value::as_f64)
            .unwrap_or(f64::from(ara_runtime::components::Light::default().range)),
    )
}
fn horizontal(value: &Value, entity: &str) -> Option<[f64; 2]> {
    let value = &value["values"][format!("{entity}.Transform.position")];
    Some([value[0].as_f64()?, value[2].as_f64()?])
}
fn moved(before: &Value, after: &Value, entity: &str) -> Option<f64> {
    let before = horizontal(before, entity)?;
    let after = horizontal(after, entity)?;
    Some(((after[0] - before[0]).powi(2) + (after[1] - before[1]).powi(2)).sqrt())
}
fn camera_moved(before: &Value, after: &Value) -> Option<f64> {
    let mut square = 0.;
    for axis in 0..3 {
        square += (after["camera"]["position"][axis].as_f64()?
            - before["camera"]["position"][axis].as_f64()?)
        .powi(2);
    }
    Some(square.sqrt())
}
impl PlayCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Play verification requires the isolated behaviour fixture".into());
        }
        std::fs::create_dir_all(output)?;
        editor.apply(vec![
            json!({"cmd":"entity.create","args":{"id":LIGHT,"name":"Play proof light","components":{"Transform":{"position":[0,3,0]},"Light":{"intensity":1000}}}}),
            json!({"cmd":"entity.create","args":{"id":ACTOR,"name":"Play proof actor","components":{"Transform":{"position":[0,0.5,0]},"MeshRenderer":{"shape":"capsule","size":[0.6,1.2,0.6]},"CharacterController":{"radius":0.3,"height":0.9,"crouchHeight":0.7},"PhysicsId":{"value":7301},"PlayerControl":{"camera":CAMERA}}}}),
            json!({"cmd":"entity.create","args":{"id":CAMERA,"name":"Play proof camera","components":{"Transform":{"position":[0,2,-5]},"Camera":{"priority":100},"FollowCamera":{"target":ACTOR,"distance":5,"height":0.6}}}}),
            json!({"cmd":"entity.create","args":{"id":"e_playprooffloor","name":"Play proof floor","components":{"Transform":{"position":[0,-0.5,0]},"MeshRenderer":{"size":[40,1,40]},"RigidBody":{},"Collider":{"halfExtents":[20,0.5,20]},"PhysicsId":{"value":7302}}}}),
        ],"Create native Play proof fixture",None)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waiting: false,
            failed_frame: false,
            began: Instant::now(),
            before: 0,
            initial: 0,
            scene_before: Value::Null,
            configuration: Value::Null,
            patch_tick: 0,
            step_began: Instant::now(),
            probe: None,
            proofs: BTreeMap::new(),
            trace: Vec::new(),
            results: Vec::new(),
            waits: Default::default(),
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
            if let Some((job, _)) = self.probe.take() {
                let _ = editor.action("jobs.cancel", json!({"job":job}));
            }
            events.clear();
            events.push((0, Event::FocusLost));
            key(events, 225, true);
            tap(events, 62);
            key(events, 225, false);
            self.results.push(json!({"journey":act.label(),"ok":false,"error":error.to_string(),"age":self.age,"trace":self.trace,"proofs":self.proofs,"initialScene":self.scene_before,"initialRevision":self.initial,"configuration":self.configuration,"waits":self.waits.values()}));
            self.advance();
            self.failed_frame = true;
            self.write()?;
        }
        Ok(())
    }
    fn wait(&mut self) -> ara_ui::Result<()> {
        if self.waits.record(
            self.age,
            self.began.elapsed(),
            || json!({"kind":"startup_or_action","selector":null,"rect":null,"clip":null}),
        ) {
            self.write()?;
        }
        if self.began.elapsed() > Duration::from_secs(180) {
            return Err(format!("Native Play story exceeded its bounded startup/action wait at age {}; last wait: {}",self.age,ara_json::to_canonical_string(self.waits.last())).into());
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
            ("play.processMode", json!("separate")),
            ("play.clients", json!(1)),
            ("play.inGameView", json!(true)),
            ("play.startPaused", json!(false)),
            // This isolated Play fixture uses the runtime/project policy; the
            // viewport override changes only the editor's Scene rendering.
            ("rendering.quality", json!("low")),
            ("viewport.cameraSpeed", json!(4.)),
        ] {
            editor.action("editor.settings.set", json!({"key":setting,"value":value}))?;
        }
        editor.apply(vec![json!({"cmd":"property.set","args":{"entity":LIGHT,"path":"Light.intensity","value":1000}}),json!({"cmd":"property.set","args":{"entity":LIGHT,"path":"Light.range","value":10}})],"Reset Play proof light",None)?;
        shell.rebuild(editor)?;
        shell.input.blur(&mut shell.ui);
        self.before = editor.host.store().revision();
        self.initial = self.before;
        self.scene_before = configuration::scene(editor)?;
        self.configuration = Value::Null;
        Ok(())
    }
    fn visible(
        &mut self,
        shell: &Shell,
        id: &str,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        if control(shell, id).is_some() {
            return Ok(true);
        }
        if self
            .waits
            .record(self.age, self.began.elapsed(), || waits::control(shell, id))
        {
            self.write()?;
        }
        if let (Some(node), Some(scroll)) = (shell.shown_node(id), shell.shown_node("play-options"))
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
        self.wait()?;
        Ok(false)
    }
    /// Read-only probes wait on Host jobs while the normal editor loop keeps drawing.
    fn probe(
        &mut self,
        editor: &mut Editor,
        method: &str,
        mut params: Value,
    ) -> ara_ui::Result<Option<Value>> {
        let Some((job, polled)) = &mut self.probe else {
            params["world"] = json!(
                editor
                    .play_world
                    .as_deref()
                    .ok_or("Play probe has no active world")?
            );
            params["player"] = json!(0);
            if method != "play.view" {
                params["background"] = json!(true);
            }
            let reply = editor.action(method, params)?;
            let job = reply["job"]
                .as_str()
                .ok_or("Readonly Play probe did not return a job")?
                .to_owned();
            self.probe = Some((job, Instant::now()));
            self.wait()?;
            return Ok(None);
        };
        if polled.elapsed() < Duration::from_millis(50) {
            self.wait()?;
            return Ok(None);
        }
        *polled = Instant::now();
        let status = editor.action("jobs.status", json!({"job":job}))?;
        match status["status"].as_str() {
            Some("done") => {
                self.probe = None;
                Ok(Some(status["result"].clone()))
            }
            Some("running") => {
                self.wait()?;
                Ok(None)
            }
            _ => Err(format!(
                "Readonly Play probe failed: {}",
                ara_json::to_canonical_string(&status["diagnostics"])
            )
            .into()),
        }
    }
    fn read_proof(&mut self, editor: &mut Editor, label: &str) -> ara_ui::Result<()> {
        let paths: Vec<_> = [ACTOR, "e_player02", "e_player03", CAMERA]
            .into_iter()
            .flat_map(|entity| {
                [
                    format!("{entity}.Transform.position"),
                    format!("{entity}.PlayerControl.player"),
                ]
            })
            .collect();
        if let Some(value) = self.probe(editor, "play.read", json!({"paths":paths}))? {
            self.proofs.insert(label.into(), value);
        }
        Ok(())
    }
    fn view_proof(&mut self, editor: &mut Editor, label: &str) -> ara_ui::Result<()> {
        if let Some(value) = self.probe(editor, "play.view", json!({"inspect":true}))? {
            self.proofs.insert(label.into(), value);
        }
        Ok(())
    }
    fn ready(&mut self, shell: &Shell, count: usize) -> ara_ui::Result<()> {
        if !shell.game_running
            || shell.play_controls.players.len() != count
            || !shell
                .play_controls
                .players
                .values()
                .all(|player| player.live)
            || shell.play_runtime_report()["pending"] == true
        {
            if self.waits.record(self.age, self.began.elapsed(), || {
                waits::players(shell, count)
            }) {
                self.write()?;
            }
            self.wait()?;
        }
        Ok(())
    }
    fn held(&mut self, milliseconds: u64) -> ara_ui::Result<bool> {
        if self.step_began.elapsed() < Duration::from_millis(milliseconds) {
            self.wait()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn stop_keys(events: &mut Vec<(u32, Event)>) {
        key(events, 26, false);
        key(events, 7, false);
        events.push((
            0,
            Event::MouseButton {
                button: 3,
                pressed: false,
            },
        ));
    }
    fn stop_play(events: &mut Vec<(u32, Event)>) {
        key(events, 225, true);
        tap(events, 62);
        key(events, 225, false);
    }
    fn multi_input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let target = match self.age {
            2 => Some("game-options"),
            4 => Some("play-options-mode"),
            5 => Some("play-client-count"),
            6 => Some("play-process-mode"),
            7 => Some("play-network-1"),
            _ => None,
        };
        if let Some(id) = target
            && !self.visible(shell, id, scale, events)?
        {
            return Ok(());
        }
        match self.age {
            0 => {
                self.began = Instant::now();
                events.push((0, Event::FocusLost));
                self.setup(editor, shell)?;
            }
            2 => click(shell, "game-options", scale, events)?,
            4 => select(shell, "play-options-mode", 1, scale, events)?,
            5 => replace(shell, "play-client-count", "2", scale, events)?,
            6 => select(
                shell,
                "play-process-mode",
                usize::from(act == Act::MultiShared),
                scale,
                events,
            )?,
            7 => select(shell, "play-network-1", 2, scale, events)?,
            8 => {
                self.configured_baseline(editor)?;
                tap(events, 62);
            }
            9 => self.ready(shell, 3)?,
            10 => {
                self.read_proof(editor, "multi-before")?;
                if !self.waiting {
                    let status = editor.action("play.status", json!({}))?;
                    let world = status["worlds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|row| {
                            row["world"] == editor.play_world.as_deref().unwrap_or_default()
                        })
                        .ok_or("Active multi-player world missing from status")?
                        .clone();
                    self.proofs.insert("processes".into(), world);
                }
            }
            11 => click(shell, "game-frame-2", scale, events)?,
            12 => {
                key(events, 26, true);
            }
            13 => {
                if self.held(450)? {
                    return Ok(());
                }
                Self::stop_keys(events);
                tap(events, 41);
            }
            14 => {
                if self.held(200)? {
                    return Ok(());
                }
                self.read_proof(editor, "multi-client2")?;
            }
            15 => click(shell, "game-frame-1", scale, events)?,
            16 => key(events, 7, true),
            17 => {
                if self.held(450)? {
                    return Ok(());
                }
                events.push((0, Event::FocusLost));
            }
            18 => {
                if self.held(200)? {
                    return Ok(());
                }
                self.read_proof(editor, "multi-client1")?;
            }
            19 => {
                if self.held(200)? {
                    return Ok(());
                }
                self.read_proof(editor, "multi-focuslost")?;
            }
            20 => {
                Self::stop_keys(events);
                tap(events, 41);
            }
            21 => Self::stop_play(events),
            22 if editor.play_world.is_some() || editor.state.playing.is_some() => {
                self.wait()?;
            }
            _ => {}
        }
        Ok(())
    }
    fn control_input(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.age > 7 && shell.play_runtime_report()["pending"] == true {
            return self.wait();
        }
        let target = match self.age {
            2 => Some("game-options"),
            4 => Some("play-process-mode"),
            18 => Some("play-possess"),
            24 => Some("play-eject"),
            _ => None,
        };
        if let Some(id) = target
            && !self.visible(shell, id, scale, events)?
        {
            return Ok(());
        }
        match self.age {
            0 => {
                self.began = Instant::now();
                events.push((0, Event::FocusLost));
                self.setup(editor, shell)?;
            }
            2 => click(shell, "game-options", scale, events)?,
            4 => select(shell, "play-process-mode", 1, scale, events)?,
            6 => {
                self.configured_baseline(editor)?;
                key(events, 226, true);
                tap(events, 62);
                key(events, 226, false);
            }
            7 => self.ready(shell, 1)?,
            8 => {
                self.view_proof(editor, "simulate-view")?;
                if !self.waiting
                    && self
                        .proofs
                        .get("simulate-view")
                        .is_none_or(|view| view["mode"] != "spectator")
                {
                    self.wait()?;
                }
            }
            9 => self.read_proof(editor, "simulate-actor")?,
            10 | 27 => {
                click(shell, "game-frame-0", scale, events)?;
                events.push((
                    0,
                    Event::MouseButton {
                        button: 3,
                        pressed: true,
                    },
                ));
                let point =
                    control(shell, "game-frame-0").ok_or("Captured Game view is missing")?;
                events.push((
                    0,
                    Event::MouseMotion {
                        position: point.map(|value| value / scale),
                        delta: [24. / scale, -12. / scale],
                    },
                ));
                key(events, 26, true);
            }
            11 => {
                if self.held(350)? {
                    return Ok(());
                }
                Self::stop_keys(events);
            }
            12 => self.view_proof(editor, "spectator-moved")?,
            14 => tap(events, 41),
            15 => self.read_proof(editor, "spectator-actor")?,
            16 => {
                editor.action("editor.search", json!({"panel":"hierarchy","query":ACTOR}))?;
                shell.rebuild(editor)?;
            }
            17 => {
                let id = shell
                    .bindings
                    .iter()
                    .find_map(|(id, binding)| {
                        matches!(binding,crate::shell::Binding::Select(entity) if entity==ACTOR)
                            .then_some(*id)
                    })
                    .ok_or("Native hierarchy row for the controllable actor is missing")?;
                let node = shell
                    .ui
                    .by_id(id)
                    .ok_or("Controllable actor hierarchy row was removed")?;
                let rect = shell.ui.nodes[node].rect;
                let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
                if !shell.ui.nodes[node].clip.contains(point[0], point[1]) {
                    return Err("Controllable actor hierarchy row is clipped".into());
                }
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
            }
            18 => click(shell, "play-possess", scale, events)?,
            20 => self.view_proof(editor, "possessed-view")?,
            21 => {
                click(shell, "game-frame-0", scale, events)?;
                key(events, 26, true);
            }
            22 => {
                if self.held(350)? {
                    return Ok(());
                }
                Self::stop_keys(events);
                tap(events, 41);
            }
            23 => self.read_proof(editor, "possessed-actor")?,
            24 => click(shell, "play-eject", scale, events)?,
            26 => self.view_proof(editor, "ejected-view")?,
            28 => {
                if self.held(200)? {
                    return Ok(());
                }
                events.push((0, Event::FocusLost));
            }
            29 => self.view_proof(editor, "focuslost-view")?,
            30 => {
                if self.held(250)? {
                    return Ok(());
                }
            }
            31 => self.view_proof(editor, "focuslost-settled-view")?,
            32 => {
                Self::stop_keys(events);
                tap(events, 41);
            }
            33 => Self::stop_play(events),
            34 if editor.play_world.is_some() || editor.state.playing.is_some() => {
                self.wait()?;
            }
            _ => {}
        }
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
        if act.multi() {
            return self.multi_input(act, editor, shell, scale, events);
        }
        if act == Act::Control {
            return self.control_input(editor, shell, scale, events);
        }
        if self.age > 9 && shell.play_runtime_report()["pending"] == true {
            return self.wait();
        }
        let target = match self.age {
            2 => Some("game-options"),
            4 => Some("play-start-paused"),
            9 | 26 => Some("play-inspect"),
            15 => Some("play-keep"),
            19 => Some("keep-fields-apply"),
            _ => None,
        };
        if let Some(target) = target
            && !self.visible(shell, target, scale, events)?
        {
            return Ok(());
        }
        match self.age {
            0 => {
                self.began = Instant::now();
                events.push((0, Event::FocusLost));
                self.setup(editor, shell)?;
            }
            2 => click(shell, "game-options", scale, events)?,
            4 => click(shell, "play-start-paused", scale, events)?,
            7 => {
                self.configured_baseline(editor)?;
                tap(events, 62);
            }
            8 => self.ready(shell, 1)?,
            9 => click(shell, "play-inspect", scale, events)?,
            10 => {
                let id = format!("runtime-{}", ara_ui::source::stable_id("Light.intensity#"));
                if !self.visible(shell, &id, scale, events)? {
                    return Ok(());
                }
                replace(shell, &id, "2500", scale, events)?;
            }
            11 => {
                let report = shell.play_runtime_report();
                if report["inspection"]["components"]["Light"]["intensity"].as_f64() != Some(2500.)
                {
                    return self.wait();
                }
            }
            12 => {
                let id = format!("runtime-{}", ara_ui::source::stable_id("Light.range#"));
                if !self.visible(shell, &id, scale, events)? {
                    return Ok(());
                }
                replace(shell, &id, "25", scale, events)?;
            }
            13 => {
                let report = shell.play_runtime_report();
                if report["inspection"]["components"]["Light"]["range"].as_f64() != Some(25.) {
                    return self.wait();
                }
                self.patch_tick = report["inspection"]["tick"].as_u64().unwrap_or(0);
            }
            15 => click(shell, "play-keep", scale, events)?,
            16 => {
                let report = shell.play_runtime_report();
                if report["keep"]["fields"].as_array().is_none() {
                    return self.wait();
                }
                if report["keep"]["selected"]
                    .as_array()
                    .is_none_or(|fields| !fields.is_empty())
                {
                    return Err(
                        "Keep preview selected fields without an explicit checkbox action".into(),
                    );
                }
                for path in ["Light.intensity", "Light.range"] {
                    if !report["keep"]["fields"].as_array().is_some_and(|fields| {
                        fields
                            .iter()
                            .any(|field| field["path"] == path && field["eligible"] == true)
                    }) {
                        return Err(format!(
                            "Keep preview did not expose the independently changed field {path}"
                        )
                        .into());
                    }
                }
            }
            17 => {
                let id = format!(
                    "keep-field-{:016x}",
                    ara_ui::source::stable_id(&format!("{LIGHT}#Light.intensity"))
                );
                if !self.visible(shell, &id, scale, events)? {
                    return Ok(());
                }
                click(shell, &id, scale, events)?;
            }
            19 => click(shell, "keep-fields-apply", scale, events)?,
            21 => {
                key(events, 224, true);
                tap(events, 29);
                key(events, 224, false);
            }
            23 => {
                key(events, 224, true);
                tap(events, 28);
                key(events, 224, false);
            }
            25 => tap(events, 64),
            26 => click(shell, "play-inspect", scale, events)?,
            28 => tap(events, 62),
            29 => tap(events, 63),
            30 => {
                key(events, 225, true);
                tap(events, 62);
                key(events, 225, false);
            }
            31 if editor.play_world.is_some() || editor.state.playing.is_some() => {
                return self.wait();
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
        let stage = if act.multi() {
            match self.age {
                3 => Some("before"),
                10 => Some("ready"),
                12 => Some("preview"),
                14 => Some("client2-moved"),
                18 => Some("released"),
                19 => Some("focus-stopped"),
                24 => Some("after"),
                _ => None,
            }
        } else if act == Act::Control {
            match self.age {
                3 => Some("before"),
                8 => Some("simulated"),
                12 => Some("spectator-moved"),
                20 => Some("possessed"),
                23 => Some("actor-moved"),
                26 => Some("ejected"),
                31 => Some("focus-stopped"),
                36 => Some("after"),
                _ => None,
            }
        } else {
            match (act, self.age) {
                (_, 3) => Some("before"),
                (Act::Runtime, 14) => Some("preview"),
                (Act::Runtime, 16) => Some("field-choices"),
                (Act::Runtime, 18) => Some("selected-one-field"),
                (Act::Runtime, 20) => Some("kept"),
                (Act::Runtime, 22) => Some("undo"),
                (Act::Runtime, 24) => Some("redo"),
                (Act::Runtime, 27) => Some("stepped"),
                (Act::Runtime, 28) => Some("resumed"),
                (Act::Runtime, 29) => Some("paused"),
                (_, age) if age == act.finish() => Some("after"),
                _ => None,
            }
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
            self.trace.push(json!({"stage":stage,"revision":editor.host.store().revision(),"authoringIntensity":intensity(editor),"authoringRange":light_range(editor),"playing":editor.state.playing,"world":editor.play_world,"runtime":shell.play_runtime_report(),"frames":frames,"proofs":self.proofs,"settings":{"mode":editor.state.settings.get("play.mode"),"processMode":editor.state.settings.get("play.processMode"),"clients":editor.state.settings.get("play.clients"),"profiles":editor.state.settings.get("play.networkProfiles")}}));
        }
        if self.age == act.finish() {
            let trace = |stage: &str| self.trace.iter().find(|trace| trace["stage"] == stage);
            let proof = |key: &str| self.proofs.get(key).unwrap_or(&Value::Null);
            let ok = if act.multi() {
                let before = proof("multi-before");
                let client2 = proof("multi-client2");
                let client1 = proof("multi-client1");
                let stopped = proof("multi-focuslost");
                let processes = proof("processes");
                let pids: BTreeSet<_> = processes["players"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|player| player["pid"].as_u64())
                    .collect();
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
                    && processes["processMode"]
                        == if act == Act::MultiShared {
                            "shared"
                        } else {
                            "separate"
                        }
                    && pids.len() == if act == Act::MultiShared { 1 } else { 3 }
                    && moved(before, client2, "e_player03").is_some_and(|distance| distance > 0.1)
                    && moved(before, client2, ACTOR).is_some_and(|distance| distance < 0.05)
                    && moved(before, client2, "e_player02").is_some_and(|distance| distance < 0.05)
                    && moved(client2, client1, "e_player02").is_some_and(|distance| distance > 0.1)
                    && moved(client2, client1, "e_player03").is_some_and(|distance| distance < 0.05)
                    && moved(client1, stopped, "e_player02").is_some_and(|distance| distance < 0.05)
                    && editor.host.store().revision() == self.before
                    && editor.play_world.is_none()
            } else if act == Act::Control {
                let initial = proof("simulate-view");
                let navigated = proof("spectator-moved");
                let idle = proof("simulate-actor");
                let spectator = proof("spectator-actor");
                let possessed = proof("possessed-actor");
                initial["mode"] == "spectator"
                    && initial["navigationSpeed"] == 4.
                    && idle["values"][format!("{ACTOR}.PlayerControl.player")] == 0
                    && camera_moved(initial, navigated).is_some_and(|distance| distance > 0.1)
                    && navigated["revision"]
                        .as_u64()
                        .zip(initial["revision"].as_u64())
                        .is_some_and(|(after, before)| after > before)
                    && moved(idle, spectator, ACTOR).is_some_and(|distance| distance < 0.05)
                    && proof("possessed-view")["mode"] == "gameplay"
                    && possessed["values"][format!("{ACTOR}.PlayerControl.player")] == 1
                    && moved(spectator, possessed, ACTOR).is_some_and(|distance| distance > 0.1)
                    && proof("ejected-view")["mode"] == "spectator"
                    && proof("focuslost-view") == proof("focuslost-settled-view")
                    && proof("focuslost-view")["mode"] == "spectator"
                    && trace("spectator-moved").is_some_and(|row| {
                        row["runtime"]["captured"] == true && row["playing"] == true
                    })
                    && trace("focus-stopped").is_some_and(|row| row["runtime"]["captured"] == false)
                    && editor.host.store().revision() == self.before
                    && editor.play_world.is_none()
                    && editor.state.playing.is_none()
            } else {
                trace("preview").is_some_and(|row| {
                    row["authoringIntensity"].as_f64() == Some(1000.)
                        && row["authoringRange"].as_f64() == Some(10.)
                        && row["runtime"]["inspection"]["components"]["Light"]["intensity"].as_f64()
                            == Some(2500.)
                        && row["runtime"]["inspection"]["components"]["Light"]["range"].as_f64()
                            == Some(25.)
                        && row["revision"] == self.before
                }) && trace("field-choices").is_some_and(|row| {
                    row["revision"] == self.before
                        && row["runtime"]["keep"]["selected"]
                            .as_array()
                            .is_some_and(Vec::is_empty)
                }) && trace("selected-one-field").is_some_and(|row| {
                    row["runtime"]["keep"]["selected"]
                        == json!([{"entity":LIGHT,"path":"Light.intensity"}])
                }) && trace("kept").is_some_and(|row| {
                    row["authoringIntensity"].as_f64() == Some(2500.)
                        && row["authoringRange"].as_f64() == Some(10.)
                        && row["revision"] == self.before + 1
                }) && trace("undo").is_some_and(|row| {
                    row["authoringIntensity"].as_f64() == Some(1000.)
                        && row["authoringRange"].as_f64() == Some(10.)
                        && row["revision"] == self.before + 2
                }) && trace("redo").is_some_and(|row| {
                    row["authoringIntensity"].as_f64() == Some(2500.)
                        && row["authoringRange"].as_f64() == Some(10.)
                        && row["revision"] == self.before + 3
                }) && trace("stepped").is_some_and(|row| {
                    row["runtime"]["inspection"]["tick"].as_u64() == Some(self.patch_tick + 1)
                }) && trace("resumed").is_some_and(|row| row["playing"] == true)
                    && trace("paused").is_some_and(|row| row["playing"] == false)
                    && editor.play_world.is_none()
                    && editor.state.playing.is_none()
            };
            self.results.push(
                json!({"journey":act.label(),"ok":ok,"trace":self.trace,"proofs":self.proofs,"configuration":self.configuration,"waits":self.waits.values()}),
            );
            self.advance();
            self.write()?;
        } else {
            self.age += 1;
            self.step_began = Instant::now();
        }
        Ok(())
    }
    fn advance(&mut self) {
        self.index += 1;
        self.age = 0;
        self.trace.clear();
        self.proofs.clear();
        self.probe = None;
        self.waits.clear();
        self.configuration = Value::Null;
        self.step_began = Instant::now();
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("play-behaviours.json"),
            ara_json::to_canonical_string(
                &json!({"journeys":self.results,"completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len(),"active":ACTS.get(self.index).map(|act|json!({"journey":act.label(),"age":self.age,"initialScene":self.scene_before,"initialRevision":self.initial,"configuration":self.configuration,"waits":self.waits.values()}))}),
            ),
        )?;
        Ok(())
    }
}
