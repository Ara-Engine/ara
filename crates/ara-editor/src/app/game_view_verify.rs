//! `--verify-game-view`: how the Game view presents its image, frame by frame, as the
//! user sees it. Offscreen only. The camera preview settles, Play starts, then the
//! dock and aspect change under the running game (sidebars collapse and return, the
//! view/bottom split moves, 4:3 and 21:9 letterboxes, Free again), then Play stops.
//! Every frame records the Game frame's node, the image's pixel size, where the image
//! was drawn and how much of the node it covers; captures of the presented Game region
//! go next to `game-view.json`. A frame is *small* when its image covers less than 98%
//! of the node (a frame from before a resize drawn at its old pixel size), *resampled*
//! when it is drawn at a size other than its own pixels, *unaligned* when a 1:1 image
//! starts between physical pixels (bilinear sampling blurs it by up to half a pixel).
use std::path::PathBuf;
use std::time::Instant;

use ara_editor_gpu::NativeUi;
use ara_ui::paint::{Box2, Draw};
use serde_json::{Value, json};

use crate::{Editor, shell::Shell};

/// What each phase does on its first frame; it then runs for its frame count.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// Show the Game tab and wait for the camera preview to settle.
    Preview,
    /// Press Play and wait until it is live and settled.
    Play,
    /// A layout or Game command under the running game.
    Change(&'static str),
    /// Stop Play.
    Stop,
}
const STEPS: &[Step] = &[
    Step::Preview,
    Step::Play,
    Step::Change("collapse-outline"),
    Step::Change("collapse-right"),
    Step::Change("expand-right"),
    Step::Change("expand-outline"),
    Step::Change("split-up"),
    Step::Change("split-back"),
    Step::Change("aspect-4x3"),
    Step::Change("aspect-21x9"),
    Step::Change("aspect-free"),
    Step::Stop,
];
/// Frames each change is followed for (captured closely at first).
const CHANGE_FRAMES: u32 = 90;
/// Live frames after Play shows its first frame.
const LIVE_FRAMES: u32 = 120;
/// Longest wait for the preview or for Play to go live.
const WAIT_SECONDS: f64 = 240.;

/// The route's state: [`GameViewCheck::step`] before each frame, [`GameViewCheck::end`]
/// after it is presented.
pub(super) struct GameViewCheck {
    output: PathBuf,
    step: usize,
    age: u32,
    started: Instant,
    /// Frames since the phase's condition held (preview shown, Play live).
    since: Option<u32>,
    rows: Vec<Value>,
}

impl GameViewCheck {
    pub fn new(output: &std::path::Path) -> Self {
        Self {
            output: output.to_owned(),
            step: 0,
            age: 0,
            started: Instant::now(),
            since: None,
            rows: Vec::new(),
        }
    }
    pub fn finished(&self) -> bool {
        self.step >= STEPS.len()
    }
    fn label(step: Step) -> String {
        match step {
            Step::Preview => "preview".into(),
            Step::Play => "play".into(),
            Step::Change(name) => name.into(),
            Step::Stop => "stop".into(),
        }
    }

    /// Before the frame: run the phase's command on its first frame. True when the
    /// shell must rebuild.
    pub fn step(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<bool> {
        let Some(&step) = STEPS.get(self.step) else {
            return Ok(false);
        };
        self.age += 1;
        if self.age > 1 {
            return Ok(false);
        }
        self.started = Instant::now();
        let target = crate::shell::commands::Target::None;
        match step {
            Step::Preview => {
                editor.action("editor.layout.show", json!({"panel": "game"}))?;
            }
            Step::Play => shell.run_command(editor, "play.start", &target)?,
            Step::Stop => shell.run_command(editor, "play.stop", &target)?,
            Step::Change(name) => match name {
                "collapse-outline" | "collapse-right" | "expand-right" | "expand-outline" => {
                    let (verb, area) = name.split_once('-').unwrap_or_default();
                    editor.action(
                        "editor.layout.collapse",
                        json!({"area": area, "collapsed": verb == "collapse"}),
                    )?;
                }
                // The Scene/Game area over the bottom dock (default 0.72).
                "split-up" | "split-back" => {
                    let ratio = if name == "split-up" { 0.5 } else { 0.72 };
                    editor.action(
                        "editor.layout",
                        json!({"path": [true, false], "ratio": ratio}),
                    )?;
                }
                aspect => {
                    let id = format!("game.aspect.{}", aspect.trim_start_matches("aspect-"));
                    shell.run_command(editor, &id, &target)?;
                }
            },
        }
        Ok(true)
    }

    /// After the frame: record the Game image's placement, capture closely after each
    /// change, and advance when the phase ends.
    pub fn end(
        &mut self,
        shell: &Shell,
        gpu: &mut NativeUi,
        presented: bool,
    ) -> ara_ui::Result<()> {
        let Some(&step) = STEPS.get(self.step) else {
            return Ok(());
        };
        let dpi = gpu.dpi();
        let shown = shown_image(shell);
        let placement = shown.and_then(|(node, texture, size)| {
            let drawn = drawn_rect(&shell.ui.display.commands, texture)?;
            Some((node, size, drawn))
        });
        let ready = match step {
            Step::Preview => shell.game_preview.is_some(),
            Step::Play => shell.game_live,
            Step::Change(_) => true,
            Step::Stop => !shell.game_live,
        };
        if ready && self.since.is_none() {
            self.since = Some(0);
        } else if let Some(since) = &mut self.since {
            *since += 1;
        }
        let since = self.since.unwrap_or(0);
        let capture_now = presented
            && placement.is_some()
            && match step {
                Step::Preview => since == 60,
                Step::Play => self.since.is_some() && (since < 30 || since % 30 == 0),
                Step::Change(_) => self.age <= 20 || self.age % 30 == 0,
                Step::Stop => false,
            };
        let label = Self::label(step);
        let capture = if capture_now {
            let name = format!("{label}-{:04}.png", self.age);
            self.save(gpu, placement.map(|p| p.0), &name)?
                .then_some(name)
        } else {
            None
        };
        if let Some((node, size, drawn)) = placement {
            let cover = coverage(node, drawn);
            let physical = [drawn.w * dpi, drawn.h * dpi];
            let resampled = (physical[0] - size[0] as f32).abs() > 0.5
                || (physical[1] - size[1] as f32).abs() > 0.5;
            let fractional = |v: f32| ((v * dpi) - (v * dpi).round()).abs() > 0.01;
            self.rows.push(json!({
                "phase": label,
                "age": self.age,
                "sinceReady": self.since,
                "presented": presented,
                "dpi": dpi,
                "node": [node.x, node.y, node.w, node.h],
                "nodePixels": [(node.w * dpi).round(), (node.h * dpi).round()],
                "target": shell.game_target,
                "image": size,
                "drawn": [drawn.x, drawn.y, drawn.w, drawn.h],
                "coverage": cover,
                "small": cover < 0.98,
                "resampled": resampled,
                "unaligned": !resampled && (fractional(drawn.x) || fractional(drawn.y)),
                "live": shell.game_live,
                "fps": shell.game_fps,
                "capture": capture,
            }));
        } else {
            self.rows.push(json!({
                "phase": label, "age": self.age, "sinceReady": self.since,
                "presented": presented, "image": Value::Null, "live": shell.game_live,
                "start": shell.game_start.as_ref().map(|s| s.phase.clone()),
                "startFailed": shell.game_start_failed, "fault": shell.game_fault,
            }));
        }
        let waited = self.started.elapsed().as_secs_f64() >= WAIT_SECONDS;
        if matches!(step, Step::Play | Step::Preview) && waited && self.since.is_none() {
            self.write(&Value::Null)?;
            return Err(format!(
                "{label} never became ready in {WAIT_SECONDS} s (start {:?}, failed {}, fault {:?})",
                shell.game_start.as_ref().map(|s| &s.phase),
                shell.game_start_failed,
                shell.game_fault
            )
            .into());
        }
        let done = match step {
            Step::Preview => since >= 90 || waited,
            Step::Play => since >= LIVE_FRAMES || waited,
            Step::Change(_) => self.age >= CHANGE_FRAMES,
            Step::Stop => since >= 30 || waited,
        };
        if done {
            self.step += 1;
            self.age = 0;
            self.since = None;
        }
        Ok(())
    }

    /// Save the presented frame's Game node region (physical pixels) as `name`.
    fn save(&self, gpu: &mut NativeUi, node: Option<Box2>, name: &str) -> ara_ui::Result<bool> {
        let Some(rect) = node else {
            return Ok(false);
        };
        let dpi = gpu.dpi();
        let pixels = gpu.pixels()?;
        let x = (rect.x * dpi).round().max(0.) as u32;
        let y = (rect.y * dpi).round().max(0.) as u32;
        let w = ((rect.w * dpi).round() as u32).min(pixels.width().saturating_sub(x));
        let h = ((rect.h * dpi).round() as u32).min(pixels.height().saturating_sub(y));
        if w == 0 || h == 0 {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.output)?;
        image::imageops::crop_imm(&pixels, x, y, w, h)
            .to_image()
            .save(self.output.join(name))?;
        Ok(true)
    }

    /// Write `game-view.json`: every frame, and per phase the frames that were small,
    /// resampled or unaligned and how many frames the image took to fit its node.
    pub fn write(&self, device: &Value) -> ara_ui::Result<()> {
        let mut phases = serde_json::Map::new();
        for row in &self.rows {
            let Some(phase) = row["phase"].as_str() else {
                continue;
            };
            let entry = phases.entry(phase.to_owned()).or_insert_with(|| {
                json!({"frames": 0, "small": 0, "resampled": 0, "unaligned": 0,
                    "minCoverage": 1.0, "fitAtAge": Value::Null})
            });
            entry["frames"] = json!(entry["frames"].as_u64().unwrap_or(0) + 1);
            for key in ["small", "resampled", "unaligned"] {
                if row[key] == true {
                    entry[key] = json!(entry[key].as_u64().unwrap_or(0) + 1);
                }
            }
            if let Some(cover) = row["coverage"].as_f64() {
                let min = entry["minCoverage"].as_f64().unwrap_or(1.).min(cover);
                entry["minCoverage"] = json!(min);
                let fits = row["resampled"] == false && row["small"] == false;
                if !fits {
                    entry["fitAtAge"] = Value::Null;
                } else if entry["fitAtAge"].is_null() {
                    entry["fitAtAge"] = row["age"].clone();
                }
            }
        }
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("game-view.json"),
            ara_json::to_canonical_string(&json!({
                "device": device,
                "phases": phases,
                "frames": self.rows,
            })),
        )?;
        Ok(())
    }
}

/// The Game image on screen: its frame node, UI texture and pixel size (the focused
/// player's live frame, else the camera preview).
fn shown_image(shell: &Shell) -> Option<(Box2, u32, [u32; 2])> {
    let node = |id: &str| shell.ui.find(id).map(|i| shell.ui.nodes[i].rect);
    if let Some(info) = shell.play_controls.players.get(&shell.game_client)
        && info.live
    {
        let rect = node(&format!("game-frame-{}", shell.game_client))?;
        return Some((
            rect,
            super::game::player_texture(shell.game_client),
            info.size,
        ));
    }
    let size = shell.game_preview?;
    Some((node("game-frame")?, super::game_preview::texture(), size))
}

fn drawn_rect(commands: &[Draw], texture: u32) -> Option<Box2> {
    commands.iter().find_map(|draw| match draw {
        Draw::Image { rect, image, .. } if *image == texture => Some(*rect),
        _ => None,
    })
}

/// Share of `node` the drawn image covers.
fn coverage(node: Box2, drawn: Box2) -> f64 {
    let w = (node.x + node.w).min(drawn.x + drawn.w) - node.x.max(drawn.x);
    let h = (node.y + node.h).min(drawn.y + drawn.h) - node.y.max(drawn.y);
    if w <= 0. || h <= 0. || node.w <= 0. || node.h <= 0. {
        return 0.;
    }
    f64::from(w * h / (node.w * node.h))
}
