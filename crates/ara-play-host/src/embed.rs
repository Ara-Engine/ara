//! Embedded Play (D19): each client's view in an editor panel through a shared GPU
//! surface, and raw input to the focused client.
//!
//! `play.start {embed: {width, height}}` asks every rendering player (not a
//! dedicated server) for a ring of exportable images (`play.embed`, before
//! `play.start`) and pulls their NT handles into this process
//! ([`SharedSurface::duplicate_from`]). Players then publish frames (`play.frame`)
//! that the owner hands back (`play.release`); `ara_gpu::shared` has the protocol.
//!
//! The editor runs the service in-process and uses [`Surfaces`] every frame, without
//! JSON: take it from [`crate::PlayService::surfaces`] before registering the service.
//! Agents use the JSON methods `play.device` and `play.resize`.
use crate::{PlayService, codes, fail, pool::Player};
use ara_diag::Diagnostic;
use ara_gpu::shared::{Frame, FrameReady, Mailbox, SharedSurface, SurfaceDesc};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::mpsc::{Receiver, Sender, TryRecvError, channel},
    time::Duration,
};

/// Wire names between the owner and embedded players (mirrored by `ara-player`).
pub mod method {
    /// Owner → player request before `play.start`: `{width, height, maxFps?}` → the
    /// surface (`SurfaceDesc`, handle values in the player's process).
    pub const EMBED: &str = "play.embed";
    /// Owner → player `{width, height}`: a request answers the new surface; a
    /// notification is answered with [`SURFACE`].
    pub const RESIZE: &str = "play.resize";
    /// Player → owner notification: a new surface generation (`SurfaceDesc`).
    pub const SURFACE: &str = "play.surface";
    /// Player → owner notification: a frame completed (`FrameReady`).
    pub const FRAME: &str = "play.frame";
    /// Owner → player notification: a frame's slot is free again (`Frame`).
    pub const RELEASE: &str = "play.release";
    /// Owner → player `{events}` raw device input (request or notification).
    pub const DEVICE: &str = "play.device";
}

/// `play.start` `embed`: render each client into a shared surface for an editor panel
/// instead of a window.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmbedParams {
    /// Surface width in pixels (the Game view's size; default 1280).
    #[ara(default = 1280, range(min = 1, max = 8192))]
    #[serde(default = "default_width")]
    pub width: u32,
    /// Surface height in pixels (default 720).
    #[ara(default = 720, range(min = 1, max = 8192))]
    #[serde(default = "default_height")]
    pub height: u32,
    /// Most frames each player renders per second (default 60).
    #[ara(range(min = 1, max = 1000))]
    pub max_fps: Option<u32>,
    /// Surface pixels per game-UI unit: the Game view's UI scale (2 on a Retina
    /// display), so the game UI keeps its size in the panel. Default 1.
    #[ara(range(min = 0.25, max = 8))]
    pub scale: Option<f32>,
}

fn default_width() -> u32 {
    1280
}

fn default_height() -> u32 {
    720
}

/// One raw device event for an embedded player, mapped to actions exactly like a
/// windowed player's input (W A S D and arrows Move, Space Jump, Left Shift Sprint,
/// right-drag Look). JSON: `{"kind": "key", "scancode": 26, "down": true}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DeviceEvent {
    /// A key changed state. Send transitions only; a press of a held key is ignored.
    Key {
        /// SDL physical scancode (W 26, A 4, S 22, D 7, Space 44, Left Shift 225).
        scancode: i32,
        /// Pressed (true) or released.
        down: bool,
    },
    /// A mouse button changed state (1 left, 2 middle, 3 right; hold 3 to look).
    MouseButton {
        /// SDL button number.
        button: u8,
        /// Pressed (true) or released.
        down: bool,
    },
    /// Relative mouse motion in pixels.
    MouseMotion {
        /// Rightward motion.
        dx: f32,
        /// Downward motion.
        dy: f32,
    },
    /// The view lost focus: every held key and button is released and look stops.
    FocusLost,
}

/// A frame's timing as the owner saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTiming {
    /// Player index (0 = the authority or local player, 1.. = clients).
    pub player: usize,
    /// The frame.
    pub frame: Frame,
    /// Render complete (before the copy into the slot) → the owner read the notice,
    /// in microseconds (wall clocks of one machine).
    pub latency_us: u64,
    /// The player's whole frame: prepare, render and copy, in microseconds.
    pub cost_us: u64,
    /// Of which the copy into the slot and its fence, in microseconds.
    pub publish_us: u64,
}

/// Service → [`Surfaces`] updates.
pub(crate) enum Update {
    /// A world's player has a (new generation) surface.
    Surface {
        world: String,
        player: usize,
        surface: SharedSurface,
    },
    /// A player published a frame.
    Frame {
        world: String,
        player: usize,
        ready: FrameReady,
        received_at_us: u64,
    },
    /// The world stopped (or restarted in new players): its surfaces are gone.
    Closed { world: String },
}

/// [`Surfaces`] → service requests, forwarded to players on the next host tick.
pub(crate) enum Request {
    Release {
        world: String,
        player: usize,
        frame: Frame,
    },
    Device {
        world: String,
        player: usize,
        events: Vec<DeviceEvent>,
    },
    Resize {
        world: String,
        player: usize,
        size: [u32; 2],
        scale: f32,
    },
}

/// The service's ends of the [`Surfaces`] channels.
pub(crate) struct Channel {
    pub updates: Sender<Update>,
    pub requests: Receiver<Request>,
}

/// What one embedded player shows in the owner's process.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceState<'a> {
    /// Player index (0 = the authority or local player, 1.. = clients in launch
    /// order), as in `play.read`'s `player`.
    pub player: usize,
    /// Changes on every resize: import [`SurfaceState::surface`] again when it
    /// differs from the generation you imported.
    pub generation: u64,
    /// Width and height in pixels.
    pub size: [u32; 2],
    /// The surface to import on the editor's device (handles owned by [`Surfaces`],
    /// valid in this process until the generation changes or the world closes).
    pub surface: &'a SharedSurface,
    /// The newest frame not shown yet ([`Surfaces::take`] it to show it).
    pub latest: Option<Frame>,
}

struct Entry {
    surface: SharedSurface,
    mailbox: Mailbox,
}

/// The editor's in-process view of embedded players: surfaces to import, the newest
/// frame of each, slot releases, raw input and resizes. Call [`Surfaces::poll`] once
/// per editor frame after the host ticked. Requests reach players on the next host
/// tick. Hold at most one taken frame per player and release it once your GPU no
/// longer samples it (with `NativeUi`, which waits for its submit, right after
/// showing the next one).
pub struct Surfaces {
    pub(crate) updates: Receiver<Update>,
    pub(crate) requests: Sender<Request>,
    worlds: BTreeMap<String, BTreeMap<usize, Entry>>,
    timings: Vec<FrameTiming>,
}

/// Recent frame timings kept for [`Surfaces::take_timings`].
const TIMINGS: usize = 1024;

impl Surfaces {
    pub(crate) fn new(updates: Receiver<Update>, requests: Sender<Request>) -> Self {
        Self {
            updates,
            requests,
            worlds: BTreeMap::new(),
            timings: Vec::new(),
        }
    }

    /// Applies the service's updates: new surfaces, frames (superseded unshown ones
    /// are released at once) and closed worlds. Returns false once the service is
    /// gone.
    pub fn poll(&mut self) -> bool {
        loop {
            match self.updates.try_recv() {
                Ok(Update::Surface {
                    world,
                    player,
                    surface,
                }) => {
                    self.worlds.entry(world).or_default().insert(
                        player,
                        Entry {
                            surface,
                            mailbox: Mailbox::default(),
                        },
                    );
                }
                Ok(Update::Frame {
                    world,
                    player,
                    ready,
                    received_at_us,
                }) => {
                    if self.timings.len() >= TIMINGS {
                        self.timings.remove(0);
                    }
                    self.timings.push(FrameTiming {
                        player,
                        frame: ready.frame,
                        latency_us: received_at_us.saturating_sub(ready.rendered_at_us),
                        cost_us: ready.cost_us,
                        publish_us: ready.publish_us,
                    });
                    let entry = self.worlds.get_mut(&world).and_then(|w| w.get_mut(&player));
                    let release = match entry {
                        // An older generation's ring is gone: nothing to return.
                        Some(entry) if entry.surface.desc().generation > ready.frame.generation => {
                            None
                        }
                        Some(entry) => entry.mailbox.deliver(ready.frame),
                        None => Some(ready.frame),
                    };
                    if let Some(frame) = release {
                        self.release(&world, player, frame);
                    }
                }
                Ok(Update::Closed { world }) => {
                    self.worlds.remove(&world);
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    self.worlds.clear();
                    return false;
                }
            }
        }
    }

    /// Worlds with embedded players (`play:1`, …).
    pub fn worlds(&self) -> impl Iterator<Item = &str> {
        self.worlds.keys().map(String::as_str)
    }

    /// The embedded players of `world`, by player index.
    pub fn surfaces(&self, world: &str) -> Vec<SurfaceState<'_>> {
        self.worlds
            .get(world)
            .map(|players| {
                players
                    .iter()
                    .map(|(&player, entry)| {
                        let desc = entry.surface.desc();
                        SurfaceState {
                            player,
                            generation: desc.generation,
                            size: [desc.width, desc.height],
                            surface: &entry.surface,
                            latest: entry.mailbox.latest(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Takes the newest unshown frame of `player` to show it. You hold it until you
    /// [`Surfaces::release`] it.
    pub fn take(&mut self, world: &str, player: usize) -> Option<Frame> {
        self.worlds.get_mut(world)?.get_mut(&player)?.mailbox.take()
    }

    /// Returns a taken frame's slot to its player, once your GPU no longer samples it.
    /// Frames of older generations or closed worlds are ignored by the player.
    pub fn release(&self, world: &str, player: usize, frame: Frame) {
        let _ = self.requests.send(Request::Release {
            world: world.to_owned(),
            player,
            frame,
        });
    }

    /// Sends raw input to exactly one player (the focused Game view).
    pub fn device(&self, world: &str, player: usize, events: Vec<DeviceEvent>) {
        if !events.is_empty() {
            let _ = self.requests.send(Request::Device {
                world: world.to_owned(),
                player,
                events,
            });
        }
    }

    /// Resizes one player's surface to `size` pixels at `scale` pixels per game-UI unit
    /// (the panel's UI scale). A new generation arrives through [`Surfaces::poll`];
    /// keep showing the old one until its first frame.
    pub fn resize(&self, world: &str, player: usize, size: [u32; 2], scale: f32) {
        let _ = self.requests.send(Request::Resize {
            world: world.to_owned(),
            player,
            size,
            scale,
        });
    }

    /// Drains the timings of frames received since the last call (at most 1024).
    pub fn take_timings(&mut self) -> Vec<FrameTiming> {
        std::mem::take(&mut self.timings)
    }
}

/// An embedded player's notice, kept in arrival order until the next host tick.
#[derive(Debug, Clone)]
pub enum Notice {
    /// A new surface generation (`play.surface`), handles in the player's process.
    Surface(SurfaceDesc),
    /// A completed frame (`play.frame`).
    Frame {
        /// The player's notice.
        ready: FrameReady,
        /// When the owner read it (Unix microseconds).
        received_at_us: u64,
    },
}

/// `play.embed` on one player before its `play.start`; its handles are then pulled
/// into this process.
pub(crate) fn embed_player(
    player: &mut Player,
    params: &EmbedParams,
) -> Result<SharedSurface, Vec<Diagnostic>> {
    let desc = match player.call(method::EMBED, &json!(params), Duration::from_secs(120)) {
        Ok(Ok(value)) => serde_json::from_value::<SurfaceDesc>(value).map_err(|e| {
            fail(
                codes::SURFACE,
                format!("play.embed returned no surface: {e}"),
            )
        })?,
        Ok(Err(error)) => {
            return Err(fail(
                codes::SURFACE,
                format!(
                    "play.embed failed: {}",
                    error["message"].as_str().unwrap_or("player error")
                ),
            ));
        }
        Err(fault) => {
            return Err(fail(
                codes::PLAYER_FAULT,
                fault["message"]
                    .as_str()
                    .unwrap_or("player fault")
                    .to_owned(),
            ));
        }
    };
    share(player, &desc)
}

/// Pulls a player's surface handles into this process (see
/// [`SharedSurface::duplicate_from`] for why the owner pulls them).
fn share(player: &Player, desc: &SurfaceDesc) -> Result<SharedSurface, Vec<Diagnostic>> {
    SharedSurface::duplicate_from(&player.child, desc).map_err(|e| {
        fail(
            codes::SURFACE,
            format!("cannot share player {}'s surface: {e}", player.hello.pid),
        )
    })
}

impl PlayService {
    /// The in-process editor's handle on embedded players ([`Surfaces`]): take it
    /// before registering the service with the host. Taking it again replaces the
    /// previous handle; embedded players then restart their rings (a same-size
    /// resize), so slots an old handle held come back.
    pub fn surfaces(&mut self) -> Surfaces {
        self.graphics_prewarm = true;
        let (updates, updates_rx) = channel();
        let (requests_tx, requests) = channel();
        self.channel = Some(Channel { updates, requests });
        let ids: Vec<String> = self.worlds.keys().cloned().collect();
        for id in ids {
            self.announce(&id);
            if let Some(world) = self.worlds.get_mut(&id) {
                for ((_, player), surface) in world.players.iter_mut().zip(&world.surfaces) {
                    if let Some(surface) = surface {
                        let desc = surface.desc();
                        let size = json!({"width": desc.width, "height": desc.height});
                        let _ = player.link.notify(method::RESIZE, &size);
                        let _ = player.link.flush();
                    }
                }
            }
        }
        Surfaces::new(updates_rx, requests_tx)
    }

    /// Sends an update to the editor handle. Without one (or once it was dropped,
    /// which disconnects it) the update comes back.
    fn send(&mut self, update: Update) -> Option<Update> {
        let unsent = match &self.channel {
            Some(channel) => channel.updates.send(update).err().map(|error| error.0),
            None => return Some(update),
        };
        if unsent.is_some() {
            self.channel = None;
        }
        unsent
    }

    /// Sends a world's current surfaces to the editor handle.
    pub(crate) fn announce(&mut self, id: &str) {
        if self.channel.is_none() {
            return;
        }
        let Some(world) = self.worlds.get(id) else {
            return;
        };
        let updates: Vec<Update> = world
            .surfaces
            .iter()
            .enumerate()
            .filter_map(|(player, surface)| {
                Some(Update::Surface {
                    world: id.to_owned(),
                    player,
                    surface: surface.as_ref()?.try_clone().ok()?,
                })
            })
            .collect();
        for update in updates {
            let _ = self.send(update);
        }
    }

    /// Tells the editor handle that a world's surfaces are gone.
    pub(crate) fn close(&mut self, id: &str) {
        let _ = self.send(Update::Closed {
            world: id.to_owned(),
        });
    }

    /// Host tick: forwards the editor's requests to players, and players' surface and
    /// frame notices to the editor. Without an editor handle, frames go straight back
    /// to their player. Returns problems to log.
    pub(crate) fn pump_embed(&mut self) -> Vec<Diagnostic> {
        let requests: Vec<Request> = self
            .channel
            .as_ref()
            .map(|c| c.requests.try_iter().collect())
            .unwrap_or_default();
        for request in requests {
            let (world, player, name, params) = match request {
                Request::Release {
                    world,
                    player,
                    frame,
                } => (world, player, method::RELEASE, json!(frame)),
                Request::Device {
                    world,
                    player,
                    events,
                } => (world, player, method::DEVICE, json!({"events": events})),
                Request::Resize {
                    world,
                    player,
                    size,
                    scale,
                } => (
                    world,
                    player,
                    method::RESIZE,
                    json!({"width": size[0], "height": size[1], "scale": scale}),
                ),
            };
            if let Some((_, p)) = self
                .worlds
                .get_mut(&world)
                .and_then(|w| w.players.get_mut(player))
            {
                let _ = p.link.notify(name, &params);
            }
        }
        let connected = self.channel.is_some();
        let mut problems = Vec::new();
        let mut updates = Vec::new();
        for (id, world) in &mut self.worlds {
            for (index, (_, player)) in world.players.iter_mut().enumerate() {
                for notice in std::mem::take(&mut player.embed) {
                    match notice {
                        Notice::Surface(desc) => match share(player, &desc) {
                            Ok(surface) => {
                                if connected && let Ok(copy) = surface.try_clone() {
                                    updates.push(Update::Surface {
                                        world: id.clone(),
                                        player: index,
                                        surface: copy,
                                    });
                                }
                                if let Some(slot) = world.surfaces.get_mut(index) {
                                    *slot = Some(surface);
                                }
                            }
                            Err(mut d) => problems.append(&mut d),
                        },
                        Notice::Frame {
                            ready,
                            received_at_us,
                        } if connected => updates.push(Update::Frame {
                            world: id.clone(),
                            player: index,
                            ready,
                            received_at_us,
                        }),
                        Notice::Frame { ready, .. } => {
                            let _ = player.link.notify(method::RELEASE, ready.frame);
                        }
                    }
                }
                let _ = player.link.flush();
            }
        }
        for update in updates {
            if let Some(Update::Frame {
                world,
                player,
                ready,
                ..
            }) = self.send(update)
                && let Some((_, p)) = self
                    .worlds
                    .get_mut(&world)
                    .and_then(|w| w.players.get_mut(player))
            {
                // The editor handle is gone: hand the slot straight back.
                let _ = p.link.notify(method::RELEASE, ready.frame);
                let _ = p.link.flush();
            }
        }
        problems
    }

    /// Fails unless player `index` of world `id` is embedded.
    fn embedded(&mut self, id: &str, index: usize) -> Result<(), Vec<Diagnostic>> {
        let world = self.world(id)?;
        if world.surfaces.get(index).is_none_or(Option::is_none) {
            return Err(fail(
                codes::REQUEST,
                format!("player {index} of `{id}` is not embedded (start Play with `embed`)"),
            ));
        }
        Ok(())
    }

    /// `play.resize`: resizes now and returns the new surface generation.
    pub(crate) fn resize(
        &mut self,
        id: &str,
        index: usize,
        size: [u32; 2],
    ) -> Result<Value, Vec<Diagnostic>> {
        self.embedded(id, index)?;
        let value = self.call_on(
            id,
            method::RESIZE,
            json!({"width": size[0], "height": size[1]}),
            Some(index),
        )?;
        let desc: SurfaceDesc = serde_json::from_value(value).map_err(|e| {
            fail(
                codes::SURFACE,
                format!("play.resize returned no surface: {e}"),
            )
        })?;
        let world = self.world(id)?;
        let surface = share(&world.players[index].1, &desc)?;
        let shown = surface.desc().clone();
        let copy = surface.try_clone().ok();
        world.surfaces[index] = Some(surface);
        if let Some(surface) = copy {
            let _ = self.send(Update::Surface {
                world: id.to_owned(),
                player: index,
                surface,
            });
        }
        Ok(json!({"world": id, "player": index, "surface": shown}))
    }

    /// `play.device`: validates the events, then sends them to exactly one player.
    pub(crate) fn device(
        &mut self,
        id: &str,
        index: usize,
        events: Vec<Value>,
    ) -> Result<Value, Vec<Diagnostic>> {
        self.embedded(id, index)?;
        let events = events
            .into_iter()
            .enumerate()
            .map(|(i, event)| {
                serde_json::from_value::<DeviceEvent>(event)
                    .map_err(|e| fail(codes::REQUEST, format!("events[{i}]: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.call_on(id, method::DEVICE, json!({"events": events}), Some(index))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ara_host::Service;

    /// The method params (MCP tool schemas) register cleanly, nested `embed` included.
    #[test]
    fn play_methods_register_valid_schemas() {
        let mut db = ara_schema::SchemaDb::new();
        let root = std::env::temp_dir();
        let service = PlayService::new(&root, crate::PlayConfig::for_project(&root));
        let methods = service.methods(&mut db);
        for name in ["play.device", "play.resize", "play.start"] {
            assert!(methods.iter().any(|m| m.name == name), "{name}");
        }
        let errors: Vec<_> = db.verify().into_iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn embed_params_default_and_device_events_validate() {
        let params: EmbedParams = serde_json::from_value(json!({"width": 320})).unwrap();
        assert_eq!(
            (params.width, params.height, params.max_fps),
            (320, 720, None)
        );
        let event: DeviceEvent =
            serde_json::from_value(json!({"kind": "mouseMotion", "dx": 1, "dy": 2})).unwrap();
        assert_eq!(event, DeviceEvent::MouseMotion { dx: 1.0, dy: 2.0 });
        assert!(
            serde_json::from_value::<DeviceEvent>(json!({"kind": "key", "scancode": 4})).is_err()
        );
        assert!(serde_json::from_value::<DeviceEvent>(json!({"kind": "wheel"})).is_err());
    }
}
