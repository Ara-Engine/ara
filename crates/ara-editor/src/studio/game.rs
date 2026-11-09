//! The Game view: a native layer (`GAME_LAYER`) beside the Scene's. Outside Play it shows
//! the scene through its gameplay camera (the legacy `game_preview`, rendered as a second
//! camera on the Scene's renderer); during Play, the embedded player's newest frame (D19).
//! A click in it while playing sends the keyboard and mouse to the game; Esc, or a click
//! outside it, gives them back.
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use anyhow::Result;
use ara_doc::prefab::Resolved;
use ara_play_host::embed::DeviceEvent;
use ara_ui::paint::{Box2, DisplayList, Draw};
use ara_ui::style::Color;

use crate::Editor;
use crate::app::game::{player::PlayerView, player_texture};
use crate::app::game_preview::{self, GameHost, Preview};
use crate::app::viewport::SceneView;
use crate::gpu::ui_device;
use crate::state::Camera;
use crate::world::DocumentView;

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
    let mut view = native(GAME_LAYER)
        .key("game/view")
        .grow(1.0)
        .w_full()
        .focusable()
        .label("Game view")
        .on_pointer(|s: &mut super::Studio, e, _| {
            if e.phase == PointerPhase::Down && s.editor.state.playing.is_some() {
                s.local.game_captured = true;
                return true;
            }
            false
        });
    let notice = |glyph, title: &str, detail: &str| {
        col()
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
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
    view
}

/// What the Game view shows, for its panel's notices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The gameplay camera's picture is on screen (outside Play).
    pub preview: bool,
    /// A player's frame is on screen (during Play).
    pub live: bool,
    /// The scene has a gameplay camera.
    pub camera: bool,
    /// Play's start phase while it starts (`play.status`: compiling, loading...).
    pub phase: Option<String>,
    /// Why Play failed to start or stopped, when it did.
    pub fault: Option<String>,
}

/// The studio's answer to the Game preview: the Game view's size and the scene's camera.
#[derive(Default)]
struct Host {
    frame: Option<[u32; 2]>,
    live: bool,
    preview: Option<[u32; 2]>,
    resolved: Arc<Resolved>,
    /// The gameplay camera, cached for the store revision it was found at.
    camera: Option<(u64, Option<Camera>)>,
    revision: u64,
}

impl GameHost for Host {
    fn game_frame(&self) -> Option<[u32; 2]> {
        self.frame
    }
    fn game_live(&self) -> bool {
        self.live
    }
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
    fn set_game_preview(&mut self, size: Option<[u32; 2]>) -> Option<[u32; 2]> {
        std::mem::replace(&mut self.preview, size)
    }
}

/// Draws the Game view into its layer on its own UI adapter. Create it with the renderer;
/// [`Self::retire`] it before the renderer drops.
pub(super) struct Game {
    layer: super::layer::Layer,
    preview: Preview,
    /// The player shown (the authority, or the local player), and its world.
    player: Option<(String, usize, PlayerView)>,
    host: Host,
    /// The image last presented and at what size: an unchanged one is not drawn again.
    shown: Option<(u32, [u32; 2])>,
    /// Keys held while input goes to the game (only transitions are sent).
    held: std::collections::BTreeSet<u16>,
    /// Where the pointer was, for relative motion while captured.
    pointer: Option<[f32; 2]>,
    /// Play's start phase and fault, read from `play.status` on a cadence.
    play: (Option<String>, Option<String>),
    checked: Option<std::time::Instant>,
}

impl Game {
    /// An adapter on the renderer's GPU.
    /// # Safety
    /// The renderer outlives this; one thread submits for both.
    pub unsafe fn new(renderer: &ara_render::Renderer) -> Result<Self> {
        let images = std::iter::once(game_preview::texture()).chain((0..=16).map(player_texture));
        Ok(Self {
            // SAFETY: forwarded caller contract.
            layer: unsafe { super::layer::Layer::new(renderer, images)? },
            preview: Preview::default(),
            player: None,
            host: Host::default(),
            shown: None,
            held: Default::default(),
            pointer: None,
            play: (None, None),
            checked: None,
        })
    }

    /// Whether the next pass has Game work: Play frames to collect, or a preview to render,
    /// settle or let go.
    pub fn wants_frame(
        &mut self,
        editor: &Editor,
        renderer: &ara_render::Renderer,
        scene: &SceneView,
    ) -> bool {
        editor.state.playing.is_some()
            || self
                .preview
                .wants_frame(editor, &mut self.host, renderer, scene)
    }

    /// One pass: collect the player's frame (during Play) or render the preview, then
    /// present whichever is on screen into `layer` at `extent` physical pixels. `layer` is
    /// None while the Game view is not laid out. `resolved` is the scene at `revision`.
    /// # Safety
    /// `layer` is the live `CAMetalLayer` of the Game view.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn frame(
        &mut self,
        renderer: &mut ara_render::Renderer,
        scene: &mut SceneView,
        editor: &mut Editor,
        view: &DocumentView,
        resolved: &Arc<Resolved>,
        revision: u64,
        layer: Option<(NonNull<c_void>, [u32; 2])>,
        scale: f32,
    ) -> Result<Status> {
        self.host.frame = layer.map(|(_, extent)| extent);
        self.host.revision = revision;
        if !Arc::ptr_eq(&self.host.resolved, resolved) {
            self.host.resolved = Arc::clone(resolved);
        }
        // A frame still on the GPU: draw nothing new this pass.
        let ready = self.layer.ready()?;
        let gpu = ui_device(renderer.gpu());
        self.host.live = false;
        let mut image = None;
        self.check_play(editor);
        // New surfaces and frames arrive through the shared mailbox.
        if let Some(surfaces) = editor.play_surfaces.as_mut() {
            surfaces.poll();
        }
        if ready {
            if let Some((world, player)) = self.playing(editor) {
                let surfaces = editor
                    .play_surfaces
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("Play has no embedded surfaces"))?;
                if self
                    .player
                    .as_ref()
                    .is_none_or(|(w, p, _)| *w != world || *p != player)
                {
                    self.close_player(Some(&*surfaces))?;
                    self.player = Some((world.clone(), player, PlayerView::default()));
                }
                if let (Some((_, _, view)), Some(extent)) = (&mut self.player, self.host.frame) {
                    view.update(
                        &world,
                        player,
                        extent,
                        scale,
                        surfaces,
                        &mut self.layer.ui,
                        gpu,
                    )
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                    self.host.live = view.info().live;
                    if self.host.live {
                        image = Some(player_texture(player));
                    }
                }
            } else {
                self.close_player(editor.play_surfaces.as_ref())?;
            }
            self.preview
                .update(
                    renderer,
                    scene,
                    editor,
                    view,
                    &mut self.host,
                    &mut self.layer.ui,
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if image.is_none() && self.host.preview.is_some() {
                image = Some(game_preview::texture());
            }
        }
        let status = Status {
            preview: self.host.preview.is_some() && !self.host.live,
            live: self.host.live,
            camera: self.host.gameplay_camera(editor).is_some(),
            phase: self.play.0.clone(),
            fault: self.play.1.clone(),
        };
        let (Some((layer, extent)), Some(image), true) = (layer, image, ready) else {
            return Ok(status);
        };
        // A player's frame changes every pass; a preview only when it re-attaches.
        if self.shown == Some((image, extent)) && !self.host.live && !self.preview_changed() {
            return Ok(status);
        }
        let logical = [extent[0] as f32 / scale, extent[1] as f32 / scale];
        let rect = Box2 {
            x: 0.0,
            y: 0.0,
            w: logical[0],
            h: logical[1],
        };
        let list = DisplayList {
            commands: vec![Draw::Image {
                node: 1,
                rect,
                image,
                tint: Color(255, 255, 255, 255),
            }],
            glyphs: Vec::new(),
        };
        // SAFETY: forwarded caller contract.
        unsafe {
            self.layer
                .draw(renderer, layer, extent, scale, &list, &[])?
        };
        self.shown = Some((image, extent));
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

    /// The preview re-attaches its texture when a newer frame completes; the paint pass
    /// samples it either way, so a settling preview is drawn every pass.
    fn preview_changed(&self) -> bool {
        self.host.preview.is_some()
    }

    /// The world and player Play shows in the Game view, while it plays there.
    fn playing(&self, editor: &Editor) -> Option<(String, usize)> {
        editor.state.playing.as_ref()?;
        let world = editor.play_world.clone()?;
        let surfaces = editor.play_surfaces.as_ref()?;
        let player = surfaces.surfaces(&world).first()?.player;
        Some((world, player))
    }

    fn close_player(&mut self, surfaces: Option<&ara_play_host::embed::Surfaces>) -> Result<()> {
        if let Some((world, player, mut view)) = self.player.take() {
            view.close(Some(&world), player, surfaces, &mut self.layer.ui)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Ok(())
    }

    /// Sends a captured key, button or motion to the player in the Game view. Returns
    /// whether it was the game's (the editor must not also handle it).
    pub fn input(&mut self, editor: &Editor, event: &stillkit::Event) -> bool {
        use stillkit::Event;
        let Some((world, player)) = self.playing(editor) else {
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
            (self.playing(editor), &editor.play_surfaces)
        {
            surfaces.device(&world, player, vec![DeviceEvent::FocusLost]);
        }
    }

    /// Releases the players and the preview; call before the renderer drops.
    pub fn retire(&mut self, editor: &mut Editor, renderer: &mut ara_render::Renderer) {
        let _ = self.close_player(editor.play_surfaces.as_ref());
        let _ = std::mem::take(&mut self.preview).destroy(renderer, &mut self.layer.ui);
        self.layer.retire(renderer);
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
