//! The Game view: a native layer (`GAME_LAYER`) beside the Scene's. Outside Play it shows
//! the scene through its gameplay camera (the legacy `game_preview`, rendered as a second
//! camera on the Scene's renderer); during Play, the embedded player's newest frame (D19).
//! A click in it while playing sends the keyboard and mouse to the game; Esc, or a click
//! outside it, gives them back.
use std::sync::Arc;

use anyhow::Result;
use ara_doc::prefab::Resolved;
use ara_play_host::embed::DeviceEvent;
use ara_ui::paint::{Box2, DisplayList, Draw};
use ara_ui::style::Color;

use crate::Editor;
use crate::gpu::ui_device;
use crate::state::Camera;
use crate::studio::device::SceneDevice;
use crate::studio::player::{PlayerView, player_texture};

/// The Game view's native layer.
pub(crate) const GAME_LAYER: &str = "game";

/// The Game panel: the layer, and a notice over it when it has nothing to show or Play
/// is waiting to be clicked into.
pub(super) fn view(s: &super::Studio) -> stillkit::el::El<super::Studio> {
    use ara_kit::{icons, ui::*};
    use stillkit::el::{Align, Justify, PointerPhase, col, native, row, text};
    let l = &s.look;
    let status = &s.local.game;
    let playing = s.editor.state.playing.is_some();
    let aspect = setting_str(s, "game.aspect", "free");
    let ratio = match aspect {
        "16x9" => Some(16.0 / 9.0),
        "16x10" => Some(16.0 / 10.0),
        "21x9" => Some(21.0 / 9.0),
        "4x3" => Some(4.0 / 3.0),
        "9x16" => Some(9.0 / 16.0),
        // A fixed resolution letterboxes to its own aspect (Play renders at its size).
        _ => resolution(aspect).map(|[w, h]| w as f32 / h as f32),
    };
    // A fixed aspect letterboxes the player inside the area (Unity's Game view).
    let fitted = ratio.and_then(|ratio| {
        let area = s.local.rects.get("game/area")?;
        let (w, h) = (area.width(), area.height());
        Some(if w / h > ratio {
            (h * ratio, h)
        } else {
            (w, w / ratio)
        })
    });
    let mut view = native(GAME_LAYER)
        .key("game/view")
        .focusable()
        .label("Game view")
        .on_pointer(|s: &mut super::Studio, e, _| {
            if e.phase == PointerPhase::Down && s.editor.state.playing.is_some() {
                s.local.game_captured = true;
                return true;
            }
            false
        });
    view = match fitted {
        Some((w, h)) => view.w(w).h(h),
        None => view.grow(1.0).w_full(),
    };
    // With nothing live to show, a notice sits on the panel's own surface, so it reads
    // whatever the view's hole would show.
    let notice = |glyph, title: &str, detail: &str| {
        col()
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(super::style::tones(l).well)
            .justify(Justify::Center)
            .align(Align::Center)
            .child(empty(l, glyph, title, detail, None))
    };
    if !playing && !status.camera {
        view = view.child(notice(
            icons::VIDEO,
            "No camera in this scene",
            "Add one (Create › Camera) to see what Play will show.",
        ));
    } else if !playing && !super::unavailable::game_preview_available() {
        view = view.child(
            col()
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .fill(super::style::tones(l).well)
                .child(super::unavailable::tile(
                    l,
                    "game/preview-unavailable",
                    "The Game preview is not available yet. Play still runs the game.",
                )),
        );
    } else if let Some(fault) = status.fault.as_deref().filter(|_| playing) {
        view = view.child(notice(icons::CIRCLE_ALERT, "Play stopped", fault));
    } else if playing && !status.live {
        view = view.child(
            col()
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .justify(Justify::Center)
                .align(Align::Center)
                .child(
                    row()
                        .gap(10.0)
                        .align(Align::Center)
                        .child(spinner(l, 16.0))
                        .child(
                            text(match status.phase.as_deref() {
                                Some(phase) => format!("Starting Play: {phase}…"),
                                None => "Starting Play…".to_owned(),
                            })
                            .text_size(12.5)
                            .color(l.c.foreground.0),
                        ),
                ),
        );
    } else if playing {
        let hint = if s.local.game_captured {
            "Esc gives the mouse and keyboard back"
        } else {
            "Click to play"
        };
        view = view.child(
            col()
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .justify(Justify::End)
                .align(Align::Center)
                .child(
                    row()
                        .h(24.0)
                        .pad_x(10.0)
                        .align(Align::Center)
                        .radius(super::style::RADIUS + 1.0)
                        .fill(super::style::tones(l).strip.with_alpha(0.9))
                        .child(text(hint).text_size(11.5).color(l.c.muted_foreground.0)),
                )
                .child(stillkit::el::el().h(12.0)),
        );
    }
    // A host and its clients side by side: each tile named, the controlled one marked;
    // a click on a tile gives it the keyboard and mouse.
    for &(player, [x, y, w, h], live) in &status.players {
        let on = player == s.play.client;
        let name = match player {
            0 => "Host".to_owned(),
            n => format!("Client {n}"),
        };
        view = view.child(
            stillkit::el::el()
                .key(format!("game/tile/{player}"))
                .absolute(x as f32, y as f32)
                .w(w as f32)
                .h(h as f32)
                .label(format!("{name}: click to control"))
                .on_pointer(move |s: &mut super::Studio, e, _| {
                    if e.phase == PointerPhase::Down {
                        s.play.client = player;
                        s.local.game_captured = true;
                        return true;
                    }
                    false
                })
                .child(
                    row()
                        .absolute(6.0, 6.0)
                        .h(20.0)
                        .pad_x(8.0)
                        .gap(6.0)
                        .align(Align::Center)
                        .radius(super::style::RADIUS)
                        .fill(if on {
                            l.c.primary.0.with_alpha(0.9)
                        } else {
                            super::style::tones(l).strip.with_alpha(0.9)
                        })
                        .child(
                            text(if live {
                                name
                            } else {
                                format!("{name} · starting…")
                            })
                            .text_size(11.0)
                            .weight(600)
                            .color(if on {
                                l.c.primary_foreground.0
                            } else {
                                l.c.muted_foreground.0
                            }),
                        ),
                ),
        );
    }
    if setting_bool(s, "game.stats") {
        let text_line = match status.fps {
            Some(fps) => format!(
                "{fps} fps · {} × {}",
                s.editor.game_size[0], s.editor.game_size[1]
            ),
            None => format!("{} × {}", s.editor.game_size[0], s.editor.game_size[1]),
        };
        view = view.child(
            row()
                .absolute(8.0, 8.0)
                .h(22.0)
                .pad_x(8.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .fill(super::style::tones(l).strip.with_alpha(0.9))
                .child(
                    text(text_line)
                        .text_size(11.0)
                        .family(mono())
                        .color(l.c.muted_foreground.0),
                ),
        );
    }
    let area = col()
        .key("game/area")
        .grow(1.0)
        .min_h(0.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .fill(stillkit::Rgba::new(0.0, 0.0, 0.0, 1.0))
        .child(view);
    col()
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar(s, aspect))
        .child(area)
}

fn setting_str<'a>(s: &'a super::Studio, key: &str, default: &'a str) -> &'a str {
    s.editor
        .state
        .settings
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or(default)
}

fn setting_bool(s: &super::Studio, key: &str) -> bool {
    s.editor
        .state
        .settings
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// The fixed resolution an aspect setting names (`1920x1080`), when it names one: Play
/// renders at exactly that size and the Game view scales it to fit, as Unity's does.
pub(super) fn resolution(aspect: &str) -> Option<[u32; 2]> {
    let (w, h) = aspect.split_once('x')?;
    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
    (w >= 320 && h >= 320).then_some([w, h])
}

/// The Game view's toolbar (Unity's): aspect, Maximize on Play, statistics.
/// The Game view's zoom: the Scale slider's value while it drags, else `game.scale`.
pub(super) fn zoom(s: &super::Studio) -> f32 {
    let value = match s.local.setting_drag {
        Some(("game.scale", v)) => v,
        _ => s
            .editor
            .state
            .settings
            .get("game.scale")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0),
    };
    value.clamp(1.0, 5.0) as f32
}

fn toolbar(s: &super::Studio, aspect: &str) -> stillkit::el::El<super::Studio> {
    use ara_kit::{icons, ui::*};
    use stillkit::el::{Align, row};
    let l = &s.look;
    const KEY: &str = "game/aspect";
    const ASPECTS: [(&str, &str); 10] = [
        ("free", "Free aspect"),
        ("16x9", "16:9"),
        ("16x10", "16:10"),
        ("21x9", "21:9"),
        ("4x3", "4:3"),
        ("9x16", "9:16 portrait"),
        ("1920x1080", "1920 × 1080"),
        ("2560x1440", "2560 × 1440"),
        ("1280x720", "1280 × 720"),
        ("1080x1920", "1080 × 1920"),
    ];
    let title = ASPECTS
        .iter()
        .find(|(k, _)| *k == aspect)
        .map_or("Free aspect", |(_, t)| t);
    let open = s.local.select.as_deref() == Some(KEY);
    let mut trigger = select(l, Size::Sm, KEY, title)
        .w(140.0)
        .expanded(open)
        .on_press(|s: &mut super::Studio| {
            s.local.select = match s.local.select.as_deref() == Some(KEY) {
                true => None,
                false => Some(KEY.into()),
            }
        });
    if open {
        let items = ASPECTS
            .iter()
            .map(|(key, name)| {
                menu_check(l, &format!("{KEY}/{key}"), name, *key == aspect).on_press(
                    move |s: &mut super::Studio| {
                        s.local.select = None;
                        super::commands::run(
                            s,
                            &format!("game.aspect.{key}"),
                            &crate::catalog::Target::None,
                        );
                    },
                )
            })
            .collect();
        trigger = trigger.overlay(
            stillkit::el::menu(menu_panel(l, items))
                .match_width()
                .on_dismiss(|s: &mut super::Studio| s.local.select = None),
        );
    }
    let flag = |id: &'static str, key: &str, glyph, label: &str| {
        super::scene_overlays::toggle(l, glyph, label, setting_bool(s, key))
            .key(format!("game/{id}"))
            .on_press(move |s: &mut super::Studio| {
                super::commands::run(s, id, &crate::catalog::Target::None)
            })
    };
    row()
        .h(super::style::TOOLBAR)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .child(trigger)
        .child(scale_control(s))
        .child(stillkit::el::el().grow(1.0))
        .child(flag(
            "game.maximize_on_play",
            "game.maximizeOnPlay",
            icons::MAXIMIZE_2,
            "Maximize on Play",
        ))
        .child(flag("game.stats", "game.stats", icons::GAUGE, "Statistics"))
}

/// Unity's Scale slider: 1x to 5x, live while it drags, kept on release (`game.scale`).
fn scale_control(s: &super::Studio) -> stillkit::el::El<super::Studio> {
    use stillkit::el::{Align, row, text};
    let l = &s.look;
    let zoom = zoom(s);
    row()
        .gap(6.0)
        .align(Align::Center)
        .child(text("Scale").text_size(11.5).color(l.c.muted_foreground.0))
        .child(
            super::style::slider(
                l,
                "game/scale".into(),
                zoom,
                (1.0, 5.0),
                false,
                move |s: &mut super::Studio, v, done| {
                    let v = (f64::from(v) * 10.0).round() / 10.0;
                    match done {
                        false => s.local.setting_drag = Some(("game.scale", v)),
                        true => {
                            s.local.setting_drag = None;
                            s.act(
                                "editor.settings.set",
                                serde_json::json!({"key": "game.scale", "value": v}),
                            );
                        }
                    }
                },
            )
            .w(80.0)
            .tooltip("Zoom into the picture"),
        )
        .child(
            text(format!("{zoom:.1}x"))
                .key("game/scale/value")
                .w(30.0)
                .text_size(11.5)
                .color(l.c.foreground.0),
        )
}

/// What the Game view shows, for its panel's notices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// A player's frame is on screen (during Play).
    pub live: bool,
    /// The scene has a gameplay camera.
    pub camera: bool,
    /// Play's start phase while it starts (`play.status`: compiling, loading...).
    pub phase: Option<String>,
    /// Why Play failed to start or stopped, when it did.
    pub fault: Option<String>,
    /// Frames presented in the last second while a player runs (the Stats readout).
    pub fps: Option<u32>,
    /// With several players (a host and its clients), each one's tile: the player, its
    /// rect in the view (design px: x, y, width, height), and whether its frame shows.
    pub players: Vec<(usize, [i32; 4], bool)>,
}

/// `count` tiles filling `size` in a grid (columns first), `gap` apart: Unreal's
/// side-by-side clients.
pub(super) fn tiles(count: usize, size: [f32; 2], gap: f32) -> Vec<[f32; 4]> {
    let count = count.max(1);
    let columns = (count as f64).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    let w = (size[0] - gap * (columns - 1) as f32) / columns as f32;
    let h = (size[1] - gap * (rows - 1) as f32) / rows as f32;
    (0..count)
        .map(|i| {
            let (c, r) = (i % columns, i / columns);
            [
                c as f32 * (w + gap),
                r as f32 * (h + gap),
                w.max(1.0),
                h.max(1.0),
            ]
        })
        .collect()
}

/// What the Game view reads from the editor: its size, whether a player shows, and the scene's
/// gameplay camera.
#[derive(Default)]
struct Host {
    frame: Option<[u32; 2]>,
    live: bool,
    resolved: Arc<Resolved>,
    /// The gameplay camera, cached for the store revision it was found at.
    camera: Option<(u64, Option<Camera>)>,
    revision: u64,
}

impl Host {
    /// The camera Play starts with, if the scene has one.
    fn gameplay_camera(&mut self, _: &Editor) -> Option<Camera> {
        if let Some((at, camera)) = self.camera
            && at == self.revision
        {
            return camera;
        }
        let transforms = crate::world::world_transforms(&self.resolved);
        let camera = crate::view_camera::gameplay(&self.resolved, &transforms);
        self.camera = Some((self.revision, camera));
        camera
    }
}

/// Draws the Game view into its layer on its own UI adapter. Create it with the renderer;
/// [`Self::retire`] it before the renderer drops.
pub(super) struct Game {
    layer: super::layer::Layer,
    /// The players shown (a host and its clients side by side, or the only one), by
    /// player number, with their world.
    players: Vec<(String, usize, PlayerView)>,
    /// The player the keyboard and mouse go to (`PlayControls::client`).
    pub client: usize,
    /// How far the picture is zoomed (`game.scale`, live while its slider drags).
    pub zoom: f32,
    host: Host,
    /// The image last presented, at what size and zoom: an unchanged one is not drawn again.
    shown: Option<(u32, [u32; 2], u32)>,
    /// Keys held while input goes to the game (only transitions are sent).
    held: std::collections::BTreeSet<u16>,
    /// Where the pointer was, for relative motion while captured.
    pointer: Option<[f32; 2]>,
    /// Play's start phase and fault, read from `play.status` on a cadence.
    play: (Option<String>, Option<String>),
    checked: Option<std::time::Instant>,
    /// Live frames presented since `.0`, and the last full second's count.
    presents: (std::time::Instant, u32, u32),
}

impl Game {
    /// An adapter on the renderer's GPU.
    /// # Safety
    /// The renderer outlives this; one thread submits for both.
    pub unsafe fn new(device: &SceneDevice) -> Result<Self> {
        let images = (0..=16).map(player_texture);
        Ok(Self {
            // SAFETY: forwarded caller contract.
            layer: unsafe { super::layer::Layer::new(device, images)? },
            players: Vec::new(),
            client: 0,
            zoom: 1.0,
            host: Host::default(),
            shown: None,
            held: Default::default(),
            pointer: None,
            play: (None, None),
            presents: (std::time::Instant::now(), 0, 0),
            checked: None,
        })
    }

    /// Draws the next pass even when its image is unchanged (the layer was hidden).
    pub fn redraw(&mut self) {
        self.shown = None;
    }

    /// Whether the next pass has Game work: Play frames to collect.
    pub fn wants_frame(&self, editor: &Editor) -> bool {
        editor.state.playing.is_some()
    }

    /// One pass: collect the player's frame (during Play), then present it into `layer` at `extent` physical pixels. `layer` is
    /// None while the Game view is not laid out. `resolved` is the scene at `revision`.
    /// # Safety
    /// `layer` is the Game view's live native layer (`present::Target`).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn frame(
        &mut self,
        device: &mut SceneDevice,
        editor: &mut Editor,
        resolved: &Arc<Resolved>,
        revision: u64,
        layer: Option<(super::present::Target, [u32; 2])>,
        scale: f32,
    ) -> Result<Status> {
        self.host.frame = layer.map(|(_, extent)| extent);
        self.host.revision = revision;
        if !Arc::ptr_eq(&self.host.resolved, resolved) {
            self.host.resolved = Arc::clone(resolved);
        }
        // A frame still on the GPU: draw nothing new this pass.
        let ready = self.layer.ready(device)?;
        let gpu = ui_device(device.gpu());
        self.host.live = false;
        // What to draw: each live player's frame in its tile, else the preview.
        let mut images: Vec<(u32, [f32; 4])> = Vec::new();
        let mut players_shown = Vec::new();
        self.check_play(editor);
        // New surfaces and frames arrive through the shared mailbox.
        if let Some(surfaces) = editor.play_surfaces.as_mut() {
            surfaces.poll();
        }
        if ready {
            if let Some((world, players)) = self.playing(editor) {
                let surfaces = editor
                    .play_surfaces
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("Play has no embedded surfaces"))?;
                // Players gone (or of another world) close; new ones open.
                let mut kept = Vec::new();
                for (w, p, mut view) in std::mem::take(&mut self.players) {
                    if w == world && players.contains(&p) {
                        kept.push((w, p, view));
                    } else {
                        view.close(Some(&w), p, Some(&*surfaces), &mut self.layer.ui)
                            .map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                }
                for p in &players {
                    if !kept.iter().any(|(_, q, _)| q == p) {
                        kept.push((world.clone(), *p, PlayerView::default()));
                    }
                }
                kept.sort_by_key(|(_, p, _)| *p);
                self.players = kept;
                if let Some(extent) = self.host.frame {
                    let logical = [extent[0] as f32 / scale, extent[1] as f32 / scale];
                    let gap = if self.players.len() > 1 { 4.0 } else { 0.0 };
                    let rects = tiles(self.players.len(), logical, gap);
                    for ((_, player, view), r) in self.players.iter_mut().zip(rects) {
                        let px = [
                            ((r[2] * scale) as u32).max(1),
                            ((r[3] * scale) as u32).max(1),
                        ];
                        view.update(
                            &world,
                            *player,
                            px,
                            scale,
                            surfaces,
                            &mut self.layer.ui,
                            gpu,
                        )
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                        let live = view.info().live;
                        self.host.live |= live;
                        if live {
                            images.push((player_texture(*player), r));
                        }
                        players_shown.push((*player, r.map(|v| v.round() as i32), live));
                    }
                }
            } else {
                self.close_player(editor.play_surfaces.as_ref())?;
            }
        }
        let status = Status {
            live: self.host.live,
            camera: self.host.gameplay_camera(editor).is_some(),
            phase: self.play.0.clone(),
            fault: self.play.1.clone(),
            fps: self.host.live.then_some(self.presents.2),
            players: if players_shown.len() > 1 {
                players_shown
            } else {
                Vec::new()
            },
        };
        let (Some((layer, extent)), Some(&(image, _)), true) = (layer, images.first(), ready)
        else {
            return Ok(status);
        };
        // A player's frame changes every pass; a preview only when it re-attaches.
        let shown = (image, extent, self.zoom.to_bits());
        if self.shown == Some(shown) && !self.host.live {
            return Ok(status);
        }
        // The Scale slider zooms about the view's centre; the layer clips what spills.
        let (zoom, centre) = (
            self.zoom.max(1.0),
            [
                extent[0] as f32 / scale / 2.0,
                extent[1] as f32 / scale / 2.0,
            ],
        );
        let list = DisplayList {
            commands: images
                .iter()
                .enumerate()
                .map(|(i, (image, r))| Draw::Image {
                    node: 1 + i as u64,
                    rect: Box2 {
                        x: centre[0] + (r[0] - centre[0]) * zoom,
                        y: centre[1] + (r[1] - centre[1]) * zoom,
                        w: r[2] * zoom,
                        h: r[3] * zoom,
                    },
                    image: *image,
                    tint: Color(255, 255, 255, 255),
                })
                .collect(),
            glyphs: Vec::new(),
        };
        // SAFETY: forwarded caller contract.
        let landed = unsafe { self.layer.draw(device, layer, extent, scale, &list, &[])? };
        // One that didn't reach the screen draws again on the next pass.
        self.shown = landed.then_some(shown);
        if self.host.live {
            let (since, count, _) = &mut self.presents;
            *count += 1;
            if since.elapsed() >= std::time::Duration::from_secs(1) {
                self.presents = (std::time::Instant::now(), 0, *count);
            }
        }
        Ok(status)
    }

    /// Reads Play's start phase and fault (`play.status`): every 100 ms while it starts,
    /// every 500 ms after, never outside Play.
    fn check_play(&mut self, editor: &mut Editor) {
        if editor.state.playing.is_none() {
            self.play = (None, None);
            self.checked = None;
            return;
        }
        let every = if self.play.0.is_some() { 100 } else { 500 };
        if self
            .checked
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(every))
        {
            return;
        }
        self.checked = Some(std::time::Instant::now());
        let status = editor
            .action("play.status", serde_json::json!({}))
            .unwrap_or_default();
        let worlds = status["worlds"].as_array().cloned().unwrap_or_default();
        let shown = worlds
            .iter()
            .find(|w| w["world"].as_str() == editor.play_world.as_deref())
            .or(worlds.last());
        let state = shown.and_then(|w| w["status"].as_str()).unwrap_or("");
        let phase = shown
            .filter(|_| state == "starting")
            .map(|w| w["phase"].as_str().unwrap_or("starting").to_owned());
        let fault = shown.and_then(|w| {
            if state == "failed" {
                return Some(
                    w["error"]
                        .as_str()
                        .or_else(|| w["diagnostics"][0]["message"].as_str())
                        .unwrap_or("the player could not start")
                        .to_owned(),
                );
            }
            w["fault"]["message"].as_str().map(str::to_owned)
        });
        if phase != self.play.0 || fault != self.play.1 {
            let (world, worlds) = (
                editor.play_world.clone(),
                editor
                    .play_surfaces
                    .as_ref()
                    .map(|s| s.worlds().map(str::to_owned).collect::<Vec<_>>()),
            );
            super::trace::log(|| {
                format!(
                    "play {state}: world {world:?}, surfaces {worlds:?}, phase {phase:?}, fault {fault:?}"
                )
            });
        }
        self.play = (phase, fault);
    }

    /// The world and players Play shows in the Game view, while it plays there.
    fn playing(&self, editor: &Editor) -> Option<(String, Vec<usize>)> {
        editor.state.playing.as_ref()?;
        let world = editor.play_world.clone()?;
        let surfaces = editor.play_surfaces.as_ref()?;
        let mut players: Vec<usize> = surfaces.surfaces(&world).iter().map(|s| s.player).collect();
        players.sort_unstable();
        players.dedup();
        (!players.is_empty()).then_some((world, players))
    }

    /// The player input goes to: the controlled client when it shows, else the first.
    fn target(&self, editor: &Editor) -> Option<(String, usize)> {
        let (world, players) = self.playing(editor)?;
        let player = match players.contains(&self.client) {
            true => self.client,
            false => players[0],
        };
        Some((world, player))
    }

    fn close_player(&mut self, surfaces: Option<&ara_play_host::embed::Surfaces>) -> Result<()> {
        for (world, player, mut view) in std::mem::take(&mut self.players) {
            view.close(Some(&world), player, surfaces, &mut self.layer.ui)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Ok(())
    }

    /// Sends a captured key, button or motion to the player in the Game view. Returns
    /// whether it was the game's (the editor must not also handle it).
    pub fn input(&mut self, editor: &Editor, event: &stillkit::Event) -> bool {
        use stillkit::Event;
        let Some((world, player)) = self.target(editor) else {
            return false;
        };
        let events = match *event {
            Event::Key(vk) => {
                if !self.held.insert(vk) {
                    return true;
                }
                scancode(vk).map(|scancode| DeviceEvent::Key {
                    scancode,
                    down: true,
                })
            }
            Event::KeyUp(vk) => {
                self.held.remove(&vk);
                scancode(vk).map(|scancode| DeviceEvent::Key {
                    scancode,
                    down: false,
                })
            }
            Event::Press(_, button) | Event::Release(_, button) => Some(DeviceEvent::MouseButton {
                button: match button {
                    stillkit::Button::Left => 1,
                    stillkit::Button::Middle => 2,
                    _ => 3,
                },
                down: matches!(event, Event::Press(..)),
            }),
            Event::Move(pointer) => {
                let at = [pointer.at.x, pointer.at.y];
                let delta = self.pointer.map(|was| [at[0] - was[0], at[1] - was[1]]);
                self.pointer = Some(at);
                delta.map(|[dx, dy]| DeviceEvent::MouseMotion { dx, dy })
            }
            _ => return false,
        };
        if let (Some(surfaces), Some(event)) = (&editor.play_surfaces, events) {
            surfaces.device(&world, player, vec![event]);
        }
        true
    }

    /// Gives the keyboard and mouse back to the editor: every held key goes up in the game.
    pub fn release(&mut self, editor: &Editor) {
        self.held.clear();
        self.pointer = None;
        if let (Some((world, player)), Some(surfaces)) =
            (self.target(editor), &editor.play_surfaces)
        {
            surfaces.device(&world, player, vec![DeviceEvent::FocusLost]);
        }
    }

    /// Releases the players; call before the device drops.
    pub fn retire(&mut self, editor: &mut Editor, device: &mut SceneDevice) {
        let _ = self.close_player(editor.play_surfaces.as_ref());
        self.layer.retire(device);
    }
}

/// The SDL scancode (USB HID usage) of a Windows virtual-key code: what players read.
fn scancode(vk: u16) -> Option<i32> {
    let vk = i32::from(vk);
    Some(match vk {
        0x41..=0x5A => vk - 0x41 + 4,
        0x31..=0x39 => vk - 0x31 + 30,
        0x30 => 39,
        0x0D => 40,
        0x1B => 41,
        0x08 => 42,
        0x09 => 43,
        0x20 => 44,
        0xBD => 45,
        0xBB => 46,
        0xDB => 47,
        0xDD => 48,
        0xDC => 49,
        0xBA => 51,
        0xDE => 52,
        0xC0 => 53,
        0xBC => 54,
        0xBE => 55,
        0xBF => 56,
        0x14 => 57,
        0x70..=0x7B => vk - 0x70 + 58,
        0x2D => 73,
        0x24 => 74,
        0x21 => 75,
        0x2E => 76,
        0x23 => 77,
        0x22 => 78,
        0x27 => 79,
        0x25 => 80,
        0x28 => 81,
        0x26 => 82,
        0x11 | 0xA2 => 224,
        0x10 | 0xA0 => 225,
        0x12 | 0xA4 => 226,
        0x5B => 227,
        0xA3 => 228,
        0xA1 => 229,
        0xA5 => 230,
        0x5C => 231,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn virtual_keys_map_to_the_scancodes_players_read() {
        // W A S D, Space and Left Shift, as `DeviceEvent::Key` documents them.
        let keys = [0x57, 0x41, 0x53, 0x44, 0x20, 0x10].map(super::scancode);
        assert_eq!(
            keys,
            [Some(26), Some(4), Some(22), Some(7), Some(44), Some(225)]
        );
    }
}
