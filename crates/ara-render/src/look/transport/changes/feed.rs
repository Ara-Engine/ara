use super::*;
use anyhow::{Result, ensure};
use std::collections::BTreeMap;

struct Slot<T> {
    generation: u64,
    value: Option<T>,
    event: Option<(u64, usize)>,
}
struct Store<T>(BTreeMap<u64, Slot<T>>);
impl<T> Default for Store<T> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}
impl<T: Copy + PartialEq> Store<T> {
    fn get(&self, id: u64) -> Option<Stamped<T>> {
        let slot = self.0.get(&id)?;
        Some(Stamped {
            generation: slot.generation,
            value: slot.value?,
        })
    }
    fn generation(&self, id: u64) -> u64 {
        self.0.get(&id).map_or(0, |s| s.generation)
    }
    fn reset(&mut self) {
        for slot in self.0.values_mut() {
            slot.value = None;
            slot.event = None;
        }
    }
    fn change(
        &mut self,
        frame: u64,
        id: u64,
        current: Option<T>,
        flags: u32,
        force: bool,
        next_index: usize,
    ) -> Result<Option<(Change<T>, Option<usize>)>> {
        let slot = match self.0.entry(id) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(_) if current.is_none() => return Ok(None),
            std::collections::btree_map::Entry::Vacant(entry) => entry.insert(Slot {
                generation: 0,
                value: None,
                event: None,
            }),
        };
        let previous = slot.value;
        if previous == current && !force {
            return Ok(None);
        }
        if previous.is_none() && current.is_none() {
            return Ok(None);
        }
        let generation = slot
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("change-feed generation exhausted"))?;
        let change = Change {
            id,
            previous_generation: slot.generation,
            generation,
            flags: flags
                | if previous.is_none() { flags::ADDED } else { 0 }
                | if current.is_none() { flags::REMOVED } else { 0 },
            previous,
            current,
        };
        slot.value = current;
        slot.generation = generation;
        let index = slot.event.filter(|(f, _)| *f == frame).map(|(_, i)| i);
        slot.event = Some((frame, index.unwrap_or(next_index)));
        Ok(Some((change, index)))
    }
    fn update(
        &mut self,
        frame: u64,
        out: &mut Vec<Change<T>>,
        id: u64,
        value: Option<T>,
        flags: u32,
    ) -> Result<()> {
        if let Some((change, index)) = self.change(frame, id, value, flags, false, out.len())? {
            if let Some(i) = index {
                merge(&mut out[i], change);
            } else {
                out.push(change);
            }
        }
        Ok(())
    }
    fn check_replay(&self, change: &Change<T>, reset: bool) -> Result<()> {
        if !reset {
            ensure!(
                self.generation(change.id) == change.previous_generation
                    && self.get(change.id).map(|s| s.value) == change.previous,
                "replay predecessor mismatch for {}",
                change.id
            );
        } else {
            ensure!(change.previous.is_none(), "reset records must start absent");
            ensure!(
                change.generation > self.generation(change.id),
                "reset would regress a replay generation"
            );
        }
        Ok(())
    }
    fn replay(&mut self, change: &Change<T>) {
        self.0.insert(
            change.id,
            Slot {
                generation: change.generation,
                value: change.current,
                event: None,
            },
        );
    }
}
fn merge<T: Copy>(first: &mut Change<T>, next: Change<T>) {
    first.generation = next.generation;
    first.current = next.current;
    first.flags |= next.flags;
}

/// Persistent snapshot/stamp tables and one reusable, deterministic frame buffer.
#[derive(Default)]
pub struct ChangeFeed {
    instances: Store<InstanceState>,
    geometries: Store<GeometryState>,
    materials: Store<MaterialState>,
    lights: Store<LightState>,
    portals: Store<PortalState>,
    sun: Store<SunState>,
    out: TransportChanges,
    last_frame: Option<u64>,
    open: bool,
}
impl ChangeFeed {
    /// Begin a strictly later render frame. The first frame is automatically a reset.
    pub fn begin_frame(&mut self, frame: u64) -> Result<()> {
        ensure!(
            self.last_frame.is_none_or(|old| frame > old),
            "change-feed frames must increase"
        );
        let first = self.last_frame.is_none();
        self.out.clear(frame);
        self.out.reset = first;
        self.last_frame = Some(frame);
        self.open = true;
        Ok(())
    }
    fn writable(&self) -> Result<()> {
        ensure!(
            self.open,
            "begin a change-feed frame before writing; finish seals it"
        );
        Ok(())
    }
    /// Invalidate publication-local identities and begin a full snapshot in the current frame.
    /// Historical stamps survive, so reused native rows never silently reuse a generation.
    pub fn reset(&mut self) -> Result<()> {
        self.writable()?;
        self.instances.reset();
        self.geometries.reset();
        self.materials.reset();
        self.lights.reset();
        self.portals.reset();
        self.sun.reset();
        self.out.clear(self.out.frame);
        self.out.reset = true;
        Ok(())
    }
    /// Current instance state, independent of the order of this frame's submissions.
    pub fn instance(&self, id: u64) -> Option<Stamped<InstanceState>> {
        self.instances.get(id)
    }
    /// Current geometry/BLAS state.
    pub fn geometry(&self, id: u64) -> Option<Stamped<GeometryState>> {
        self.geometries.get(id)
    }
    /// Current material state.
    pub fn material(&self, id: u64) -> Option<Stamped<MaterialState>> {
        self.materials.get(id)
    }
    /// Current local-light state.
    pub fn light(&self, id: u64) -> Option<Stamped<LightState>> {
        self.lights.get(id)
    }
    /// Current portal state.
    pub fn portal(&self, id: u64) -> Option<Stamped<PortalState>> {
        self.portals.get(id)
    }
    /// Current sun state.
    pub fn sun(&self) -> Option<Stamped<SunState>> {
        self.sun.get(0)
    }

    /// Submit an instance; deformation is local displacement since its last submitted pose.
    /// Extra flags report paint/decal/material changes even when the snapshot is identical.
    pub fn set_instance(
        &mut self,
        id: u64,
        current: InstanceState,
        deformation: f32,
        extra_flags: u32,
    ) -> Result<()> {
        self.writable()?;
        validate_instance(current)?;
        ensure!(
            deformation.is_finite() && deformation >= 0. && extra_flags & !flags::ALL == 0,
            "invalid instance displacement/flags"
        );
        let previous = self.instance(id).map(|s| s.value);
        let mut causes = extra_flags;
        if let Some(old) = previous {
            if old.world != current.world {
                causes |= flags::MOVED;
            }
            if old.geometry != current.geometry
                || old.bounds != current.bounds
                || old.casts_shadows != current.casts_shadows
            {
                causes |= flags::SHAPE;
            }
            if old.material != current.material {
                causes |= flags::MATERIAL;
            }
            if old.body != current.body {
                causes |= flags::BODY;
                if current.body == BodyClass::Sleeping {
                    causes |= flags::SLEEP;
                }
                if old.body == BodyClass::Sleeping {
                    causes |= flags::WAKE;
                }
            }
        }
        if deformation > 0. {
            causes |= flags::DEFORMED;
        }
        if current.wind > 0. || previous.is_some_and(|s| s.wind > 0.) {
            causes |= flags::WIND;
        }
        let motion = previous.map_or(
            MotionBounds {
                swept: current.world_bounds(),
                displacement: 0.,
            },
            |old| motion_bounds(old, current, deformation),
        );
        ensure!(
            motion.swept.valid() && motion.displacement.is_finite(),
            "instance motion bounds overflow"
        );
        if let Some((change, index)) = self.instances.change(
            self.out.frame,
            id,
            Some(current),
            causes,
            causes != 0,
            self.out.instances.len(),
        )? {
            self.instance_event(change, index, motion);
        }
        Ok(())
    }
    fn instance_event(
        &mut self,
        change: Change<InstanceState>,
        index: Option<usize>,
        motion: MotionBounds,
    ) {
        if let Some(i) = index {
            let first = &mut self.out.instances[i];
            merge(&mut first.change, change);
            first.swept = first.swept.union(motion.swept);
            first.displacement += motion.displacement;
        } else {
            self.out.instances.push(InstanceChange {
                change,
                swept: motion.swept,
                displacement: motion.displacement,
            });
        }
    }
    /// Remove an instance, retaining its final world bounds for invalidation.
    pub fn remove_instance(&mut self, id: u64) -> Result<()> {
        self.writable()?;
        if let Some(old) = self.instance(id) {
            let motion = MotionBounds {
                swept: old.value.world_bounds(),
                displacement: 0.,
            };
            if let Some((change, index)) = self.instances.change(
                self.out.frame,
                id,
                None,
                0,
                false,
                self.out.instances.len(),
            )? {
                self.instance_event(change, index, motion);
            }
        }
        Ok(())
    }
    /// Submit a geometry/vertex revision, or its removal.
    pub fn set_geometry(&mut self, id: u64, value: Option<GeometryState>) -> Result<()> {
        self.writable()?;
        if let Some(v) = value {
            validate_geometry(v)?;
        }
        let causes = match (self.geometry(id), value) {
            (Some(old), Some(v)) => {
                if old.value.key != v.key {
                    flags::SHAPE
                } else {
                    flags::DEFORMED
                }
            }
            _ => 0,
        };
        self.geometries
            .update(self.out.frame, &mut self.out.geometries, id, value, causes)
    }
    /// Submit a native material block or its removal.
    pub fn set_material(&mut self, id: u64, value: Option<MaterialState>) -> Result<()> {
        self.writable()?;
        let mut causes = flags::MATERIAL;
        if let (Some(old), Some(v)) = (self.material(id), value)
            && (old.value.0[..3] != v.0[..3] || old.value.0[4..7] != v.0[4..7])
        {
            causes |= flags::RECOLOURED;
        }
        self.materials
            .update(self.out.frame, &mut self.out.materials, id, value, causes)
    }
    /// Submit a local light. Colour/intensity flags alone never require visibility invalidation.
    pub fn set_light(&mut self, id: u64, value: Option<LightState>) -> Result<()> {
        self.writable()?;
        if let Some(v) = value {
            ensure!(
                v.0[..24].iter().all(|v| f32::from_bits(*v).is_finite()),
                "non-finite light"
            );
        }
        let mut causes = 0;
        if let (Some(old), Some(v)) = (self.light(id), value) {
            let a = old.value.0;
            let b = v.0;
            if a[..3] != b[..3] || a[8..11] != b[8..11] || a[16..19] != b[16..19] {
                causes |= flags::MOVED;
            }
            if a[3] != b[3] || a[11..15] != b[11..15] || a[19..] != b[19..] {
                causes |= flags::SHAPE;
            }
            if a[4..7] != b[4..7] {
                causes |= flags::RECOLOURED;
            }
            if a[7] != b[7] || a[15] != b[15] {
                causes |= flags::INTENSITY;
            }
        }
        self.lights
            .update(self.out.frame, &mut self.out.lights, id, value, causes)
    }

    /// A profile/emission-mask payload changed behind unchanged native light-table offsets.
    /// Call after submitting the light's current record; missing/removed lights are a no-op.
    pub fn invalidate_light_shape(&mut self, id: u64) -> Result<()> {
        self.writable()?;
        if let Some(value) = self.light(id)
            && let Some((change, index)) = self.lights.change(
                self.out.frame,
                id,
                Some(value.value),
                flags::SHAPE,
                true,
                self.out.lights.len(),
            )?
        {
            if let Some(i) = index {
                merge(&mut self.out.lights[i], change);
            } else {
                self.out.lights.push(change);
            }
        }
        Ok(())
    }
    /// Submit portal openness or removal from the host's optical portal system.
    pub fn set_portal(&mut self, id: u64, value: Option<PortalState>) -> Result<()> {
        self.writable()?;
        if let Some(v) = value {
            ensure!(
                v.openness.is_finite() && (0. ..=1.).contains(&v.openness),
                "invalid portal openness"
            );
        }
        self.portals.update(
            self.out.frame,
            &mut self.out.portals,
            id,
            value,
            flags::PORTAL,
        )
    }
    /// Submit a unit sun direction and lux, or remove the sun.
    pub fn set_sun(&mut self, value: Option<SunState>) -> Result<()> {
        self.writable()?;
        if let Some(v) = value {
            validate_sun(v)?;
        }
        let old = self.sun().map(|s| s.value);
        let mut causes = 0;
        let mut angle = 0.;
        if let (Some(a), Some(b)) = (old, value) {
            if a.direction != b.direction {
                causes |= flags::MOVED;
                angle = a
                    .direction
                    .cross(b.direction)
                    .length()
                    .atan2(a.direction.dot(b.direction));
            }
            if a.lux != b.lux {
                causes |= flags::RECOLOURED | flags::INTENSITY;
            }
        }
        if let Some((change, index)) =
            self.sun
                .change(self.out.frame, 0, value, causes, false, 0)?
        {
            let elevation = value
                .or(old)
                .map_or(0., |s| s.direction.y.clamp(-1., 1.).asin());
            if index.is_some() {
                let first = self
                    .out
                    .sun
                    .as_mut()
                    .expect("sun slot references this frame's event");
                merge(&mut first.change, change);
                first.angular_delta += angle;
                first.elevation = elevation;
            } else {
                self.out.sun = Some(SunChange {
                    change,
                    angular_delta: angle,
                    elevation,
                });
            }
        }
        Ok(())
    }
    /// Seal and sort this frame; repeated reads are free. Begin a new frame before writing.
    pub fn finish(&mut self) -> &TransportChanges {
        if self.open {
            self.out.instances.sort_unstable_by_key(|c| c.change.id);
            self.out.geometries.sort_unstable_by_key(|c| c.id);
            self.out.materials.sort_unstable_by_key(|c| c.id);
            self.out.lights.sort_unstable_by_key(|c| c.id);
            self.out.portals.sort_unstable_by_key(|c| c.id);
            self.open = false;
        }
        &self.out
    }
    /// Last sealed events, without sorting or mutating storage.
    pub fn changes(&self) -> &TransportChanges {
        &self.out
    }
    /// Validate then atomically replay a frame into the same cache-facing state and event API.
    pub fn apply_replay(&mut self, frame: &TransportChanges) -> Result<()> {
        replay::validate_frame(frame)?;
        ensure!(
            self.last_frame.is_none_or(|old| frame.frame > old),
            "replay frames must increase"
        );
        ensure!(
            self.last_frame.is_some() || frame.reset,
            "replay must begin with a full reset snapshot"
        );
        for c in &frame.instances {
            self.instances.check_replay(&c.change, frame.reset)?;
        }
        for c in &frame.geometries {
            self.geometries.check_replay(c, frame.reset)?;
        }
        for c in &frame.materials {
            self.materials.check_replay(c, frame.reset)?;
        }
        for c in &frame.lights {
            self.lights.check_replay(c, frame.reset)?;
        }
        for c in &frame.portals {
            self.portals.check_replay(c, frame.reset)?;
        }
        if let Some(c) = &frame.sun {
            self.sun.check_replay(&c.change, frame.reset)?;
        }
        self.begin_frame(frame.frame)?;
        if frame.reset {
            self.reset()?;
        }
        for c in &frame.instances {
            self.instances.replay(&c.change);
        }
        for c in &frame.geometries {
            self.geometries.replay(c);
        }
        for c in &frame.materials {
            self.materials.replay(c);
        }
        for c in &frame.lights {
            self.lights.replay(c);
        }
        for c in &frame.portals {
            self.portals.replay(c);
        }
        if let Some(c) = &frame.sun {
            self.sun.replay(&c.change);
        }
        self.out.copy_from(frame);
        self.open = false;
        Ok(())
    }
}
pub(super) fn validate_instance(v: InstanceState) -> Result<()> {
    ensure!(
        v.bounds.valid()
            && v.world.is_finite()
            && v.world.x_axis.w == 0.
            && v.world.y_axis.w == 0.
            && v.world.z_axis.w == 0.
            && v.world.w_axis.w == 1.
            && v.wind.is_finite()
            && v.wind >= 0.,
        "invalid instance snapshot"
    );
    Ok(())
}
pub(super) fn validate_geometry(v: GeometryState) -> Result<()> {
    ensure!(
        v.bounds.valid() && v.displacement.is_finite() && v.displacement >= 0.,
        "invalid geometry bounds/displacement"
    );
    Ok(())
}
pub(super) fn validate_sun(v: SunState) -> Result<()> {
    ensure!(
        v.direction.is_finite()
            && (v.direction.length_squared() - 1.).abs() < 1e-4
            && v.lux.is_finite()
            && v.lux.min_element() >= 0.,
        "invalid sun snapshot"
    );
    Ok(())
}
