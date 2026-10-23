//! Rooms: reverb zones, portals between them and the room-acoustics model that turns
//! volume and absorption into reverb parameters when no baked data exists.

use crate::{
    material::MaterialPreset,
    math::{V3, add, conjugate, cross, distance, dot, length, normalize_or, rotate, scale, sub},
    propagation::AcousticsSettings,
    reverb::{EarlyTap, MAX_TAPS, ReverbParams},
};
use ara_audio::{AudioError, AudioListener, Result};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Statistical room model (diffuse field).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomAcoustics {
    /// Air volume in m³.
    pub volume: f32,
    /// Total surface area in m².
    pub surface_area: f32,
    /// Area-weighted mean absorption for low/mid/high bands.
    pub absorption: [f32; 3],
}

impl RoomAcoustics {
    /// Mean free path 4V/S, in meters.
    pub fn mean_free_path(&self) -> f32 {
        4.0 * self.volume / self.surface_area.max(1e-3)
    }

    /// Eyring reverberation time per band including air absorption:
    /// `T = 0.161·V / (−S·ln(1−α) + 8·m·V)`, clamped to [0.05, 20] s.
    pub fn rt60(&self, settings: &AcousticsSettings) -> [f32; 3] {
        std::array::from_fn(|b| {
            let alpha = self.absorption[b].clamp(1e-4, 0.999);
            let surfaces = -self.surface_area * (1.0 - alpha).ln();
            // Energy attenuation is twice the amplitude coefficient.
            let air = 8.0 * settings.air_coefficients[b] * self.volume;
            (0.161 * self.volume / (surfaces + air).max(1e-6)).clamp(0.05, 20.0)
        })
    }

    /// Reverberant-to-direct energy ratio at 1 m: 16π(1−α)/(Sα), clamped to [0.01, 4].
    pub fn level(&self) -> f32 {
        let alpha = self.absorption[1].clamp(1e-3, 0.999);
        (16.0 * std::f32::consts::PI * (1.0 - alpha) / (self.surface_area * alpha).max(1e-3))
            .clamp(0.01, 4.0)
    }
}

/// Reverb zone volume shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "audio")]
#[serde(rename_all = "camelCase")]
pub enum ZoneShape {
    /// Oriented box with `halfExtents`.
    #[default]
    Box,
    /// Sphere with `radius`.
    Sphere,
}

/// A room or space with its own reverb. The listener's zone drives the room reverb;
/// sources in other zones reach the listener through portals.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[cfg_attr(feature = "ecs", derive(bevy_ecs::component::Component))]
#[ara(component, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct ReverbZone {
    /// Unique zone name; portals, traces and diagnostics refer to zones by name.
    pub name: String,
    /// Volume shape.
    pub shape: ZoneShape,
    /// World-space center, supplied from the entity transform by the bridge.
    #[ara(unit = Meters)]
    pub position: [f32; 3],
    /// World rotation as a unit quaternion (x, y, z, w), supplied by the bridge.
    #[ara(default = [0, 0, 0, 1])]
    pub rotation: [f32; 4],
    /// Box half extents.
    #[ara(unit = Meters, default = [5, 3, 5])]
    pub half_extents: [f32; 3],
    /// Sphere radius.
    #[ara(unit = Meters, default = 5, range(min = 0.1, max = 10000))]
    pub radius: f32,
    /// Width inside the boundary over which the listener crossfades to the outer space.
    #[ara(unit = Meters, default = 1, range(min = 0, max = 100))]
    pub fade: f32,
    /// Nested zones: the highest priority containing zone wins.
    pub priority: i32,
    /// Wall material, deriving absorption for the realtime room model.
    pub material: MaterialPreset,
    /// Explicit low/mid/high reverberation times in seconds, overriding derived and baked values.
    pub rt60: Option<[f32; 3]>,
    /// Reverb return gain.
    #[ara(default = 1, range(min = 0, max = 4))]
    pub wet: f32,
    /// Geometric early reflections from the zone's walls.
    #[ara(default = true)]
    pub early_reflections: bool,
    /// Prefer baked probe reverb times inside this zone when cooked cells are loaded.
    #[ara(default = true)]
    pub use_baked: bool,
}

impl Default for ReverbZone {
    fn default() -> Self {
        Self {
            name: String::new(),
            shape: ZoneShape::Box,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            half_extents: [5.0, 3.0, 5.0],
            radius: 5.0,
            fade: 1.0,
            priority: 0,
            material: MaterialPreset::Generic,
            rt60: None,
            wet: 1.0,
            early_reflections: true,
            use_baked: true,
        }
    }
}

/// An opening (door, window, archway) connecting two zones. An empty zone name is outdoors.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[cfg_attr(feature = "ecs", derive(bevy_ecs::component::Component))]
#[ara(component, module = "audio")]
#[serde(rename_all = "camelCase")]
pub struct AcousticPortal {
    /// First zone's name; empty means outdoors.
    pub a: String,
    /// Second zone's name; empty means outdoors.
    pub b: String,
    /// World-space center of the opening, supplied by the bridge.
    #[ara(unit = Meters)]
    pub position: [f32; 3],
    /// Opening width.
    #[ara(unit = Meters, default = 1, range(min = 0.01, max = 1000))]
    pub width: f32,
    /// Opening height.
    #[ara(unit = Meters, default = 2, range(min = 0.01, max = 1000))]
    pub height: f32,
    /// How open it is: 0 closed (blocks), 1 fully open. Drive from door state.
    #[ara(default = 1, range(min = 0, max = 1))]
    pub open: f32,
}

impl Default for AcousticPortal {
    fn default() -> Self {
        Self {
            a: String::new(),
            b: String::new(),
            position: [0.0; 3],
            width: 1.0,
            height: 2.0,
            open: 1.0,
        }
    }
}

/// Where the listener is: its zone, the space around it and the crossfade weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListenerRoom {
    /// Innermost (highest-priority) containing zone; None outdoors.
    pub zone: Option<usize>,
    /// The space outside `zone` at the listener's position; None outdoors.
    pub outer: Option<usize>,
    /// Weight of `zone` against `outer`: 1 deep inside, falling to 0.5 at the boundary.
    pub weight: f32,
}

/// The best propagation path between two zones through portals.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// Portal indices from the source side to the listener side.
    pub portals: Vec<usize>,
    /// Path length source → portals → listener, in meters.
    pub length: f32,
    /// Position the sound appears to come from: the last portal's center.
    pub apparent: V3,
    /// Low/mid/high gains from openness, aperture and diffraction angles.
    pub gains: [f32; 3],
}

struct Zone {
    desc: ReverbZone,
    inverse: [f32; 4],
    room: RoomAcoustics,
    id: u64,
}

struct Portal {
    desc: AcousticPortal,
    a: usize,
    b: usize,
}

/// Validated zones and portals for one world snapshot. Rebuild when components change.
pub struct ZoneSet {
    zones: Vec<Zone>,
    portals: Vec<Portal>,
    settings: AcousticsSettings,
}

/// FNV-1a: stable room identity from a zone name.
fn room_id(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

impl ZoneSet {
    /// Validates names (unique, non-empty), shapes and portal endpoints.
    pub fn new(
        zones: &[ReverbZone],
        portals: &[AcousticPortal],
        settings: AcousticsSettings,
    ) -> Result<Self> {
        if !settings.is_valid() {
            return Err(AudioError::Invalid("acoustics settings"));
        }
        let mut built = Vec::with_capacity(zones.len());
        for (i, z) in zones.iter().enumerate() {
            let q = z.rotation;
            let qlen = (q.iter().map(|v| v * v).sum::<f32>()).sqrt();
            let finite = crate::math::finite(z.position)
                && crate::math::finite(z.half_extents)
                && z.radius.is_finite()
                && z.fade.is_finite()
                && z.wet.is_finite()
                && (qlen - 1.0).abs() < 1e-3;
            if z.name.is_empty()
                || zones[..i].iter().any(|o| o.name == z.name)
                || !finite
                || z.half_extents.iter().any(|h| *h <= 0.0)
                || z.radius <= 0.0
                || z.fade < 0.0
                || z.rt60
                    .is_some_and(|t| t.iter().any(|v| !v.is_finite() || *v <= 0.0 || *v > 30.0))
            {
                return Err(AudioError::Invalid(
                    "reverb zones need unique names, finite positive extents and a unit rotation",
                ));
            }
            let [x, y, w] = z.half_extents;
            let (volume, surface_area) = match z.shape {
                ZoneShape::Box => (8.0 * x * y * w, 8.0 * (x * y + y * w + x * w)),
                ZoneShape::Sphere => (
                    4.0 / 3.0 * std::f32::consts::PI * z.radius.powi(3),
                    4.0 * std::f32::consts::PI * z.radius * z.radius,
                ),
            };
            built.push(Zone {
                desc: z.clone(),
                inverse: conjugate(z.rotation),
                room: RoomAcoustics {
                    volume,
                    surface_area,
                    absorption: z.material.material().absorption,
                },
                id: room_id(&z.name),
            });
        }
        let outdoors = built.len();
        let find = |name: &str| -> Option<usize> {
            if name.is_empty() {
                Some(outdoors)
            } else {
                zones.iter().position(|z| z.name == name)
            }
        };
        let mut links = Vec::with_capacity(portals.len());
        for p in portals {
            let (Some(a), Some(b)) = (find(&p.a), find(&p.b)) else {
                return Err(AudioError::Invalid("portal names an unknown zone"));
            };
            if a == b
                || !crate::math::finite(p.position)
                || !p.open.is_finite()
                || !(0.0..=1.0).contains(&p.open)
                || !(p.width > 0.0 && p.height > 0.0)
            {
                return Err(AudioError::Invalid("portal endpoints, opening or size"));
            }
            links.push(Portal {
                desc: p.clone(),
                a,
                b,
            });
        }
        Ok(Self {
            zones: built,
            portals: links,
            settings,
        })
    }

    /// Number of zones.
    pub fn len(&self) -> usize {
        self.zones.len()
    }
    /// True without zones.
    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }
    /// A zone's authored component.
    pub fn zone(&self, index: usize) -> &ReverbZone {
        &self.zones[index].desc
    }
    /// A zone's statistical room model.
    pub fn room(&self, index: usize) -> RoomAcoustics {
        self.zones[index].room
    }
    /// A portal's authored component.
    pub fn portal(&self, index: usize) -> &AcousticPortal {
        &self.portals[index].desc
    }
    /// Zone name, or `outdoors`.
    pub fn name(&self, zone: Option<usize>) -> &str {
        zone.map_or("outdoors", |z| self.zones[z].desc.name.as_str())
    }

    fn local(&self, zone: usize, point: V3) -> V3 {
        let z = &self.zones[zone];
        rotate(z.inverse, sub(point, z.desc.position))
    }

    /// Signed distance inside the zone boundary (positive inside).
    pub fn depth(&self, zone: usize, point: V3) -> f32 {
        let z = &self.zones[zone].desc;
        let p = self.local(zone, point);
        match z.shape {
            ZoneShape::Box => (0..3)
                .map(|a| z.half_extents[a] - p[a].abs())
                .fold(f32::INFINITY, f32::min),
            ZoneShape::Sphere => z.radius - length(p),
        }
    }

    /// Highest-priority zone containing `point`, ties broken by smaller volume then order.
    pub fn zone_at(&self, point: V3) -> Option<usize> {
        self.containing(point, None)
    }

    fn containing(&self, point: V3, except: Option<usize>) -> Option<usize> {
        (0..self.zones.len())
            .filter(|&i| Some(i) != except && self.depth(i, point) >= 0.0)
            .min_by(|&a, &b| {
                let (za, zb) = (&self.zones[a], &self.zones[b]);
                zb.desc
                    .priority
                    .cmp(&za.desc.priority)
                    .then(za.room.volume.total_cmp(&zb.room.volume))
                    .then(a.cmp(&b))
            })
    }

    /// The listener's zone, the outer space and the boundary crossfade weight.
    pub fn listener(&self, point: V3) -> ListenerRoom {
        let Some(zone) = self.zone_at(point) else {
            return ListenerRoom {
                zone: None,
                outer: None,
                weight: 1.0,
            };
        };
        let fade = self.zones[zone].desc.fade;
        let depth = self.depth(zone, point);
        let weight = if fade <= 0.0 {
            1.0
        } else {
            0.5 + 0.5 * (depth / fade).clamp(0.0, 1.0)
        };
        ListenerRoom {
            zone: Some(zone),
            outer: self.containing(point, Some(zone)),
            weight,
        }
    }

    /// Late-reverb and early-reflection parameters for a zone heard by `listener`.
    /// `rt60` overrides the room model (baked probes); the zone's own override wins.
    pub fn params(
        &self,
        zone: usize,
        listener: &AudioListener,
        rt60: Option<[f32; 3]>,
    ) -> ReverbParams {
        let z = &self.zones[zone];
        let mut params = ReverbParams {
            rt60: z
                .desc
                .rt60
                .or(rt60)
                .unwrap_or_else(|| z.room.rt60(&self.settings)),
            mean_free_path: z.room.mean_free_path(),
            level: z.room.level(),
            wet: z.desc.wet,
            early_damping: z.room.absorption[2],
            room: z.id,
            ..ReverbParams::silent()
        };
        if z.desc.early_reflections {
            self.early(zone, listener, &mut params);
        }
        params
    }

    /// First- and second-order image sources of the zone's walls, with the source
    /// co-located at the listener. Panned in listener space.
    fn early(&self, zone: usize, listener: &AudioListener, params: &mut ReverbParams) {
        let z = &self.zones[zone];
        let half = match z.desc.shape {
            ZoneShape::Box => z.desc.half_extents,
            // Equal-volume cube for spheres.
            ZoneShape::Sphere => [z.desc.radius * 0.806; 3],
        };
        let p = self.local(zone, listener.position);
        let forward = normalize_or(listener.forward, [0.0, 0.0, -1.0]);
        let right = normalize_or(cross(forward, listener.up), [1.0, 0.0, 0.0]);
        let reflect = (1.0 - z.room.absorption[1]).max(0.0).sqrt();
        let c = self.settings.speed_of_sound;
        // Wall distance and outward axis direction, per axis and side.
        let wall = |axis: usize, side: f32| -> (f32, V3) {
            let d = (half[axis] - side * p[axis]).max(0.05);
            let mut dir = [0.0; 3];
            dir[axis] = side;
            (d, dir)
        };
        let mut push = |path: f32, local_dir: V3, order: i32| {
            if params.early_count >= MAX_TAPS {
                return;
            }
            let world = rotate(z.desc.rotation, normalize_or(local_dir, [0.0, 0.0, -1.0]));
            let pan = dot(world, right).clamp(-1.0, 1.0);
            let gain = (reflect.powi(order) / path.max(1.0)).min(0.7);
            params.early[params.early_count] = EarlyTap {
                delay: path / c,
                gain: [
                    gain * ((1.0 - pan) * 0.5).sqrt(),
                    gain * ((1.0 + pan) * 0.5).sqrt(),
                ],
            };
            params.early_count += 1;
        };
        for axis in 0..3 {
            for side in [-1.0, 1.0] {
                let (d, dir) = wall(axis, side);
                push(2.0 * d, dir, 1);
            }
        }
        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            for sa in [-1.0, 1.0] {
                for sb in [-1.0, 1.0] {
                    let (da, va) = wall(a, sa);
                    let (db, vb) = wall(b, sb);
                    let path = 2.0 * (da * da + db * db).sqrt();
                    push(path, add(scale(va, da), scale(vb, db)), 2);
                }
            }
        }
    }

    /// Shortest open path between the source's and listener's spaces through at most
    /// `max_portal_depth` portals. None when both are in the same space or nothing connects them.
    pub fn route(&self, source: V3, listener: V3) -> Option<Route> {
        let outdoors = self.zones.len();
        let from = self.zone_at(source).unwrap_or(outdoors);
        let to = self.zone_at(listener).unwrap_or(outdoors);
        if from == to {
            return None;
        }
        let mut best: Option<(f32, Vec<usize>)> = None;
        let mut stack = Vec::new();
        self.search(from, to, source, listener, 0.0, &mut stack, &mut best);
        let (length, portals) = best?;
        let mut gains = [1.0_f32; 3];
        let mut previous = source;
        for (k, &i) in portals.iter().enumerate() {
            let p = &self.portals[i].desc;
            let next = portals
                .get(k + 1)
                .map_or(listener, |&n| self.portals[n].desc.position);
            let incoming = normalize_or(sub(p.position, previous), [0.0, 0.0, 1.0]);
            let outgoing = normalize_or(sub(next, p.position), incoming);
            let angle = dot(incoming, outgoing).clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
            let aperture = (p.width * p.height / 2.0).min(1.0).sqrt();
            for (b, k) in [0.3_f32, 0.55, 0.8].into_iter().enumerate() {
                gains[b] *= p.open * aperture * (1.0 - angle * k).clamp(0.05, 1.0);
            }
            previous = p.position;
        }
        let apparent = portals
            .last()
            .map_or(source, |&i| self.portals[i].desc.position);
        Some(Route {
            portals,
            length,
            apparent,
            gains,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        at: usize,
        to: usize,
        position: V3,
        listener: V3,
        travelled: f32,
        stack: &mut Vec<usize>,
        best: &mut Option<(f32, Vec<usize>)>,
    ) {
        if at == to {
            let total = travelled + distance(position, listener);
            if best.as_ref().is_none_or(|(b, _)| total < *b) {
                *best = Some((total, stack.clone()));
            }
            return;
        }
        if stack.len() >= self.settings.max_portal_depth as usize {
            return;
        }
        for (i, p) in self.portals.iter().enumerate() {
            if p.desc.open <= 0.0 || stack.contains(&i) {
                continue;
            }
            let next = if p.a == at {
                p.b
            } else if p.b == at {
                p.a
            } else {
                continue;
            };
            stack.push(i);
            let step = distance(position, p.desc.position);
            self.search(
                next,
                to,
                p.desc.position,
                listener,
                travelled + step,
                stack,
                best,
            );
            stack.pop();
        }
    }
}
