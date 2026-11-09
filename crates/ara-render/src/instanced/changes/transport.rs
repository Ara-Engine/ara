//! Native upload adapter for the render-v3 event source.
//!
//! Public core API and v1 replay format are documented in [`ChangeFeed`] and
//! [`ReplayReader`]. Renderer capture is explicitly enabled; legacy GPU uploads do not consume
//! these events. Native IDs for instances/geometries/materials are scene row indices, scoped by
//! `TransportChanges::reset`; `InstanceState::entity` retains the external draw identity.
//! Supply `TransportHints::light_ids` when lights have stable external IDs. Without them light
//! rows are identities, and a count change conservatively removes/recreates every light row.
//!
//! `NativeChangeFeed::begin_frame` groups multiple uploads (pose, arrival fade, placeholder
//! changes) into one frame. `finish` seals it. An upload after sealing automatically begins the
//! next frame; explicitly begin idle frames too so a capture records their true empty demand.
//! Replay injection supplies the exact cache event stream. The caller must separately restore
//! scene/pose payloads by content key/revision before tracing or rendering the replay.

use super::Changes;
use crate::instanced::Scene;
pub use crate::look::transport::changes::*;
use anyhow::{Result, ensure};

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;

/// Additional producer data absent from the legacy row-only update contract.
#[derive(Default)]
pub struct TransportHints {
    /// Changed publication-local instance body classes. The adapter retains these until reset.
    /// Fill from real motion type and awake state; a stationary transform does not prove sleep.
    pub bodies: Vec<(usize, BodyClass)>,
    /// Changed absolute world-space wind amplitudes per instance, in metres. Retained until reset.
    /// Native foliage currently has no shader wind; its default is zero, not an invented bound.
    pub wind: Vec<(usize, f32)>,
    /// Geometry row and measured maximum local vertex displacement for this vertex revision.
    /// Missing bounds conservatively use the farthest pair of old/new local-box corners.
    pub deformations: Vec<(usize, f32)>,
    /// Optional full light-ID list, in current scene light order. IDs must be unique.
    /// Empty means reuse known stable IDs for unchanged counts, or publication-local rows.
    pub light_ids: Vec<u64>,
    /// Light rows whose profile/emission-mask payload changed behind the same table offsets.
    pub reshaped_lights: Vec<usize>,
    /// Optical portal updates/removals. The current RenderFrame has no optical portal state.
    pub portals: Vec<(u64, Option<PortalState>)>,
}
impl TransportHints {
    /// Clear per-upload inputs without releasing allocations.
    pub fn clear(&mut self) {
        self.bodies.clear();
        self.wind.clear();
        self.deformations.clear();
        self.light_ids.clear();
        self.reshaped_lights.clear();
        self.portals.clear();
    }
}

#[derive(Clone, Copy)]
struct Metadata {
    body: BodyClass,
    declared: bool,
    wind: f32,
}

#[derive(PartialEq)]
struct DecalSnapshot {
    entity: u64,
    world: glam::Mat4,
    material: usize,
    opacity: [f32; 7],
    opacity_channel: u32,
    occlusion: [f32; 2],
    specular: f32,
    distance: [f32; 2],
    angle: [f32; 2],
    order: i32,
    screen: f32,
}
impl From<&crate::instanced::Decal> for DecalSnapshot {
    fn from(d: &crate::instanced::Decal) -> Self {
        Self {
            entity: d.entity,
            world: d.world,
            material: d.material,
            opacity: d.opacity,
            opacity_channel: d.opacity_channel,
            occlusion: d.occlusion,
            specular: d.specular,
            distance: d.fade_distance,
            angle: d.fade_angle,
            order: d.sort_order,
            screen: d.fade_screen_size,
        }
    }
}

/// CPU-only adapter; snapshot publication builds reverse geometry-to-instance lists once.
pub struct NativeChangeFeed {
    feed: ChangeFeed,
    metadata: Vec<Metadata>,
    geometry_instances: Vec<Vec<usize>>,
    dirty: Vec<usize>,
    reasons: Vec<u32>,
    displacement: Vec<f32>,
    geometry_rows: Vec<usize>,
    material_rows: Vec<usize>,
    light_ids: Vec<u64>,
    next_lights: Vec<u64>,
    wind_instances: Vec<usize>,
    // Immutable paint handles let snapshot callers detect edits without hashing vertices.
    paints: Vec<Option<std::sync::Arc<[u32]>>>,
    decals: Vec<DecalSnapshot>,
    frame: u64,
    open: bool,
    replaying: bool,
}
impl NativeChangeFeed {
    /// Create the initial full snapshot at a caller-chosen render frame number.
    pub fn new(scene: &Scene, frame: u64) -> Result<Self> {
        let mut out = Self {
            feed: ChangeFeed::default(),
            metadata: Vec::new(),
            geometry_instances: Vec::new(),
            dirty: Vec::new(),
            reasons: Vec::new(),
            displacement: Vec::new(),
            geometry_rows: Vec::new(),
            material_rows: Vec::new(),
            light_ids: Vec::new(),
            next_lights: Vec::new(),
            wind_instances: Vec::new(),
            paints: Vec::new(),
            decals: Vec::new(),
            frame,
            open: false,
            replaying: false,
        };
        out.feed.begin_frame(frame)?;
        out.open = true;
        out.replace(scene)?;
        Ok(out)
    }
    /// Start a later frame, including frames with no upload work.
    pub fn begin_frame(&mut self, frame: u64) -> Result<()> {
        ensure!(
            !self.replaying,
            "replay frames are supplied through apply_replay"
        );
        self.feed.begin_frame(frame)?;
        self.frame = frame;
        self.open = true;
        Ok(())
    }
    fn ensure_open(&mut self) -> Result<()> {
        if !self.open {
            self.begin_frame(
                self.frame
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("render frame exhausted"))?,
            )?;
        }
        Ok(())
    }
    /// Publish replacement layout with a reset. Existing identity generation counters survive.
    pub fn replace(&mut self, scene: &Scene) -> Result<()> {
        if self.replaying {
            return Ok(());
        }
        self.ensure_open()?;
        self.feed.reset()?;
        self.metadata.clear();
        self.geometry_instances.clear();
        self.wind_instances.clear();
        self.light_ids.clear();
        self.paints.clear();
        self.decals.clear();
        self.decals
            .extend(scene.decals.iter().map(DecalSnapshot::from));
        self.dirty.clear();
        self.reasons.clear();
        self.displacement.clear();
        self.geometry_instances
            .resize_with(scene.geometry.len(), Vec::new);
        self.reasons.resize(scene.instances.len(), 0);
        self.displacement.resize(scene.instances.len(), 0.);
        for (id, geometry) in scene.geometry.iter().enumerate() {
            self.feed.set_geometry(
                id as u64,
                Some(GeometryState {
                    key: geometry.key,
                    revision: geometry.revision,
                    bounds: Aabb::new(geometry.bounds[0], geometry.bounds[1]),
                    displacement: 0.,
                }),
            )?;
        }
        for (id, material) in scene.materials.iter().enumerate() {
            self.feed
                .set_material(id as u64, Some(MaterialState::from(material)))?;
        }
        for (id, instance) in scene.instances.iter().enumerate() {
            ensure!(
                instance.geometry < scene.geometry.len()
                    && instance.material < scene.materials.len(),
                "invalid native feed instance layout"
            );
            self.geometry_instances[instance.geometry].push(id);
            self.metadata.push(Metadata {
                body: if scene.geometry[instance.geometry].deforming {
                    BodyClass::Kinematic
                } else {
                    BodyClass::Static
                },
                declared: false,
                wind: 0.,
            });
            self.paints
                .push(instance.paint.as_ref().map(std::sync::Arc::clone));
            self.feed
                .set_instance(id as u64, self.instance_state(scene, id), 0., 0)?;
        }
        self.update_lights(scene, &[])?;
        self.feed.set_sun(Some(SunState {
            direction: scene.sun_direction.normalize(),
            lux: scene.sun_lux,
        }))?;
        Ok(())
    }
    fn instance_state(&self, scene: &Scene, row: usize) -> InstanceState {
        let i = &scene.instances[row];
        let g = &scene.geometry[i.geometry];
        let metadata = self.metadata[row];
        InstanceState {
            entity: i.entity,
            geometry: i.geometry as u64,
            material: i.material as u64,
            world: i.world,
            bounds: Aabb::new(g.bounds[0], g.bounds[1]),
            body: metadata.body,
            casts_shadows: i.casts_shadows,
            wind: metadata.wind,
        }
    }
    fn mark(&mut self, row: usize, causes: u32) {
        if self.reasons[row] == 0 {
            self.dirty.push(row);
        }
        self.reasons[row] |= causes | (1 << 31);
    }
    /// Check metadata before native GPU writes. Snapshot/layout validation remains the renderer's.
    pub fn validate(&self, scene: &Scene, changes: Option<&Changes>) -> Result<()> {
        if self.replaying {
            return Ok(());
        }
        ensure!(
            scene.instances.len() == self.metadata.len()
                && scene.geometry.len() == self.geometry_instances.len(),
            "native change-feed layout changed without replacement"
        );
        if let Some(c) = changes {
            ensure!(
                c.instances
                    .iter()
                    .chain(&c.paint)
                    .all(|&i| i < scene.instances.len())
                    && c.geometries.iter().all(|&i| i < scene.geometry.len())
                    && c.materials.iter().all(|&i| i < scene.materials.len()),
                "invalid change-feed row"
            );
            ensure!(
                c.transport
                    .bodies
                    .iter()
                    .all(|&(i, _)| i < scene.instances.len())
                    && c.transport
                        .wind
                        .iter()
                        .all(|&(i, amplitude)| i < scene.instances.len()
                            && amplitude.is_finite()
                            && amplitude >= 0.),
                "invalid body/wind metadata"
            );
            ensure!(
                c.transport
                    .deformations
                    .iter()
                    .all(|&(i, bound)| i < scene.geometry.len()
                        && bound.is_finite()
                        && bound >= 0.),
                "invalid deformation metadata"
            );
            let ids = &c.transport.light_ids;
            ensure!(
                ids.is_empty() || ids.len() == scene.lights.len(),
                "light identity count mismatch"
            );
            for (i, id) in ids.iter().enumerate() {
                ensure!(!ids[..i].contains(id), "duplicate light identity");
            }
            ensure!(
                c.transport
                    .reshaped_lights
                    .iter()
                    .all(|&i| i < scene.lights.len()),
                "invalid reshaped light row"
            );
            for (_, value) in &c.transport.portals {
                ensure!(
                    value
                        .is_none_or(|p| p.openness.is_finite() && (0. ..=1.).contains(&p.openness)),
                    "invalid portal openness"
                );
            }
        }
        Ok(())
    }
    /// Append a successful upload; explicit rows avoid traversing static instance/vertex data.
    pub fn update(&mut self, scene: &Scene, changes: Option<&Changes>) -> Result<()> {
        if self.replaying {
            return Ok(());
        }
        self.validate(scene, changes)?;
        self.ensure_open()?;
        self.geometry_rows.clear();
        self.material_rows.clear();
        let decals_changed = changes.map_or_else(
            || {
                self.decals.len() != scene.decals.len()
                    || self
                        .decals
                        .iter()
                        .zip(&scene.decals)
                        .any(|(old, new)| *old != DecalSnapshot::from(new))
            },
            |c| c.decals,
        );
        if let Some(c) = changes {
            self.geometry_rows.extend_from_slice(&c.geometries);
            self.material_rows.extend_from_slice(&c.materials);
            for &i in &c.instances {
                self.mark(i, 0);
            }
            for &i in &c.paint {
                self.mark(i, flags::PAINT);
            }
            for &(i, body) in &c.transport.bodies {
                self.metadata[i].body = body;
                self.metadata[i].declared = true;
                self.mark(i, 0);
            }
            for &(i, amplitude) in &c.transport.wind {
                self.metadata[i].wind = amplitude;
                if amplitude > 0. && !self.wind_instances.contains(&i) {
                    self.wind_instances.push(i);
                }
                self.mark(i, 0);
            }
            self.wind_instances.retain(|&i| self.metadata[i].wind > 0.);
            for &(id, state) in &c.transport.portals {
                self.feed.set_portal(id, state)?;
            }
        } else {
            self.geometry_rows.extend(0..scene.geometry.len());
            self.material_rows.extend(0..scene.materials.len());
            for i in 0..scene.instances.len() {
                let paint_changed = match (&self.paints[i], &scene.instances[i].paint) {
                    (Some(old), Some(new)) => !std::sync::Arc::ptr_eq(old, new) && old != new,
                    (None, None) => false,
                    _ => true,
                };
                self.mark(i, if paint_changed { flags::PAINT } else { 0 });
            }
        }
        if decals_changed {
            for i in 0..scene.instances.len() {
                self.mark(i, flags::DECALS);
            }
            self.decals.clear();
            self.decals
                .extend(scene.decals.iter().map(DecalSnapshot::from));
        }
        // Declared wind remains active even when the host has no transform or skin delta.
        for n in 0..self.wind_instances.len() {
            self.mark(self.wind_instances[n], flags::WIND);
        }
        self.geometry_rows.sort_unstable();
        self.geometry_rows.dedup();
        for n in 0..self.geometry_rows.len() {
            let id = self.geometry_rows[n];
            let geometry = &scene.geometry[id];
            let previous = self
                .feed
                .geometry(id as u64)
                .expect("published geometry has a feed snapshot")
                .value;
            let bounds = Aabb::new(geometry.bounds[0], geometry.bounds[1]);
            if previous.key == geometry.key
                && previous.revision == geometry.revision
                && previous.bounds == bounds
            {
                continue;
            }
            let displacement = changes
                .and_then(|c| {
                    c.transport
                        .deformations
                        .iter()
                        .rev()
                        .find(|&&(i, _)| i == id)
                        .map(|&(_, bound)| bound)
                })
                .unwrap_or_else(|| {
                    (previous.bounds.min - bounds.max)
                        .abs()
                        .max((previous.bounds.max - bounds.min).abs())
                        .length()
                });
            self.feed.set_geometry(
                id as u64,
                Some(GeometryState {
                    key: geometry.key,
                    revision: geometry.revision,
                    bounds,
                    displacement,
                }),
            )?;
            for n in 0..self.geometry_instances[id].len() {
                let i = self.geometry_instances[id][n];
                self.displacement[i] = displacement;
                self.mark(i, flags::DEFORMED);
            }
        }
        self.material_rows.sort_unstable();
        self.material_rows.dedup();
        for &id in &self.material_rows {
            self.feed
                .set_material(id as u64, Some(MaterialState::from(&scene.materials[id])))?;
        }
        self.dirty.sort_unstable();
        for n in 0..self.dirty.len() {
            let id = self.dirty[n];
            if !self.metadata[id].declared
                && self.metadata[id].body == BodyClass::Static
                && self
                    .feed
                    .instance(id as u64)
                    .is_some_and(|old| old.value.world != scene.instances[id].world)
            {
                self.metadata[id].body = BodyClass::Kinematic;
            }
            self.feed.set_instance(
                id as u64,
                self.instance_state(scene, id),
                self.displacement[id],
                self.reasons[id] & !(1 << 31),
            )?;
            if self.reasons[id] & flags::PAINT != 0 {
                self.paints[id] = scene.instances[id]
                    .paint
                    .as_ref()
                    .map(std::sync::Arc::clone);
            }
            self.reasons[id] = 0;
            self.displacement[id] = 0.;
        }
        self.dirty.clear();
        self.update_lights(
            scene,
            changes.map_or(&[], |c| c.transport.light_ids.as_slice()),
        )?;
        if let Some(c) = changes {
            for &row in &c.transport.reshaped_lights {
                self.feed.invalidate_light_shape(self.light_ids[row])?;
            }
        }
        self.feed.set_sun(Some(SunState {
            direction: scene.sun_direction.normalize(),
            lux: scene.sun_lux,
        }))?;
        Ok(())
    }
    fn update_lights(&mut self, scene: &Scene, supplied: &[u64]) -> Result<()> {
        self.next_lights.clear();
        if !supplied.is_empty() {
            self.next_lights.extend_from_slice(supplied);
        } else if self.light_ids.len() == scene.lights.len() {
            self.next_lights.extend_from_slice(&self.light_ids);
        } else {
            // Without stable host IDs, a topology change cannot safely match surviving rows.
            for &id in &self.light_ids {
                self.feed.set_light(id, None)?;
            }
            self.next_lights
                .extend((0..scene.lights.len()).map(|i| i as u64));
        }
        for &id in &self.light_ids {
            if !self.next_lights.contains(&id) {
                self.feed.set_light(id, None)?;
            }
        }
        for (&id, light) in self.next_lights.iter().zip(&scene.lights) {
            self.feed.set_light(id, Some(LightState::from(light)))?;
        }
        std::mem::swap(&mut self.light_ids, &mut self.next_lights);
        Ok(())
    }
    /// Sort/seal the current per-frame stream. Read before beginning the next frame.
    pub fn finish(&mut self) -> &TransportChanges {
        self.open = false;
        self.feed.finish()
    }
    /// A1's current instance lookup (world, swept frame lookup via `finish`, generation).
    pub fn instance(&self, id: u64) -> Option<Stamped<InstanceState>> {
        self.feed.instance(id)
    }
    /// Current instance/geometry/material/light/portal/sun stamps for cache validation.
    /// Use `finish` for ordered per-frame events rather than reading unsealed events here.
    pub fn state(&self) -> &ChangeFeed {
        &self.feed
    }
    /// Inject recorded events without resynthesizing motion, stamps or sun angles.
    /// Live uploads are ignored by the feed in replay mode, so the lab can restore matching
    /// geometry/material payloads before injecting the recorded events. Re-enable to capture live.
    pub fn apply_replay(&mut self, changes: &TransportChanges) -> Result<()> {
        if self.replaying {
            self.feed.apply_replay(changes)?;
        } else {
            let mut replay = ChangeFeed::default();
            replay.apply_replay(changes)?;
            self.feed = replay;
            self.replaying = true;
        }
        self.frame = changes.frame;
        self.open = false;
        Ok(())
    }
}
