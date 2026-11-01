//! A3 ARACHG01 events are generated once and consumed by both renderer runs.
use super::{contract::Case, fixture, geometry};
use anyhow::{Result, ensure};
use ara_render::instanced::{Instance, Scene, transport::*};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
};

pub fn record(case: &Case, output: impl Write) -> Result<()> {
    let mut fixture = fixture::create(case)?;
    let mut feed = ChangeFeed::default();
    let mut writer = ReplayWriter::new(output)?;
    let mut previous = BTreeSet::new();
    for frame in case.script(60) {
        fixture::apply(&mut fixture, case, &frame);
        feed.begin_frame(u64::from(frame.frame))?;
        for (id, g) in fixture.scene.geometry.iter().enumerate() {
            feed.set_geometry(
                id as u64,
                Some(GeometryState {
                    key: g.key,
                    revision: g.revision,
                    bounds: Aabb::new(g.bounds[0], g.bounds[1]),
                    displacement: 0.,
                }),
            )?;
        }
        for (id, m) in fixture.scene.materials.iter().enumerate() {
            feed.set_material(id as u64, Some(MaterialState::from(m)))?;
        }
        for (id, l) in fixture.scene.lights.iter().enumerate() {
            feed.set_light(id as u64, Some(LightState::from(l)))?;
        }
        let current: BTreeSet<_> = fixture.scene.instances.iter().map(|i| i.entity).collect();
        for id in previous.difference(&current) {
            feed.remove_instance(*id)?;
        }
        for instance in &fixture.scene.instances {
            let g = &fixture.scene.geometry[instance.geometry];
            let body = if instance.entity >= 10_000 {
                BodyClass::PhysicsActive
            } else if (100..100 + u64::from(case.bodies)).contains(&instance.entity) {
                if case.recipe == "wind" {
                    BodyClass::Kinematic
                } else if frame.event_age.is_none()
                    || (case.recipe == "sleep_wake" && (28..=55).contains(&frame.frame))
                {
                    BodyClass::Sleeping
                } else {
                    BodyClass::PhysicsActive
                }
            } else if instance.entity == 10 && case.recipe == "door" {
                BodyClass::Kinematic
            } else {
                BodyClass::Static
            };
            let wind = if case.recipe == "wind" && body == BodyClass::Kinematic {
                0.25
            } else {
                0.
            };
            feed.set_instance(
                instance.entity,
                InstanceState {
                    entity: instance.entity,
                    geometry: instance.geometry as u64,
                    material: instance.material as u64,
                    world: instance.world,
                    bounds: Aabb::new(g.bounds[0], g.bounds[1]),
                    body,
                    casts_shadows: instance.casts_shadows,
                    wind,
                },
                0.,
                if wind > 0. { flags::WIND } else { 0 },
            )?;
        }
        if case.recipe == "door" {
            feed.set_portal(
                1,
                Some(PortalState {
                    openness: (frame.event_age.unwrap_or(0) as f32 / 60. * 3.
                        / std::f32::consts::FRAC_PI_2)
                        .min(1.),
                }),
            )?;
        }
        feed.set_sun(Some(SunState {
            direction: fixture.scene.sun_direction,
            lux: fixture.scene.sun_lux,
        }))?;
        writer.write_frame(feed.finish())?;
        previous = current;
    }
    writer.finish()?;
    Ok(())
}

pub struct Replay<R> {
    reader: ReplayReader<R>,
    feed: ChangeFeed,
    events: TransportChanges,
}
impl<R: Read> Replay<R> {
    pub fn new(input: R) -> Result<Self> {
        Ok(Self {
            reader: ReplayReader::new(input, 64 * 1024 * 1024)?,
            feed: ChangeFeed::default(),
            events: TransportChanges::default(),
        })
    }
    pub fn apply(&mut self, frame: u32, scene: &mut Scene) -> Result<&TransportChanges> {
        ensure!(
            self.reader.read_frame(&mut self.events)? && self.events.frame == u64::from(frame),
            "missing/misaligned replay frame"
        );
        self.feed.apply_replay(&self.events)?;
        if self.events.reset {
            scene.instances.clear();
        }
        for instance in &mut scene.instances {
            instance.previous_world = instance.world;
        }
        for change in &self.events.geometries {
            if let Some(g) = change.current {
                let index = usize::try_from(change.id)?;
                if index == scene.geometry.len() {
                    let triangles = u32::from_le_bytes(g.key[..4].try_into()?);
                    scene.geometry.push(geometry::cube(triangles));
                }
                ensure!(
                    scene.geometry.get(index).is_some_and(
                        |current| current.key == g.key && current.revision == g.revision
                    ),
                    "replay requires external geometry payload"
                );
            }
        }
        for change in &self.events.instances {
            if let Some(value) = change.change.current {
                if let Ok(index) = scene
                    .instances
                    .binary_search_by_key(&change.change.id, |i| i.entity)
                {
                    let instance = &mut scene.instances[index];
                    instance.world = value.world;
                    instance.geometry = usize::try_from(value.geometry)?;
                    instance.material = usize::try_from(value.material)?;
                    instance.casts_shadows = value.casts_shadows;
                } else {
                    scene.instances.push(Instance {
                        entity: value.entity,
                        name: format!("fixture-{}", value.entity),
                        geometry: usize::try_from(value.geometry)?,
                        material: usize::try_from(value.material)?,
                        world: value.world,
                        previous_world: value.world,
                        casts_shadows: value.casts_shadows,
                        paint: None,
                        receives_decals: true,
                    });
                }
            } else {
                scene.instances.retain(|i| i.entity != change.change.id);
            }
        }
        for change in &self.events.materials {
            if let Some(m) = change.current {
                let index = usize::try_from(change.id)?;
                ensure!(index < scene.materials.len(), "missing material payload");
                scene.materials[index] = bytemuck::pod_read_unaligned(bytemuck::cast_slice(&m.0));
            }
        }
        for change in &self.events.lights {
            if let Some(l) = change.current {
                let index = usize::try_from(change.id)?;
                ensure!(index < scene.lights.len(), "missing light payload");
                scene.lights[index] = bytemuck::pod_read_unaligned(bytemuck::cast_slice(&l.0));
            }
        }
        if let Some(change) = &self.events.sun
            && let Some(s) = change.change.current
        {
            scene.sun_direction = s.direction;
            scene.sun_lux = s.lux;
        }
        Ok(&self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Suite;
    #[test]
    fn recorded_destruction_and_sleep_wake_reproduce_geometry_and_generations() {
        let suite: Suite = serde_json::from_str(include_str!(
            "../../../ara-dev/src/lab/stress/cases.v1.json"
        ))
        .unwrap();
        for id in [
            "wall-breach",
            "body-sleep-wake",
            "door-opening",
            "repeated-invalidation",
            "combined-stress",
        ] {
            let case = suite.cases.iter().find(|c| c.id == id).unwrap();
            let mut bytes = Vec::new();
            record(case, &mut bytes).unwrap();
            let mut replay = Replay::new(bytes.as_slice()).unwrap();
            let mut expected = fixture::create(case).unwrap();
            let mut actual = fixture::create(case).unwrap();
            let mut saw_sleep = false;
            let mut saw_wake = false;
            for frame in case.script(60) {
                fixture::apply(&mut expected, case, &frame);
                let events = replay.apply(frame.frame, &mut actual.scene).unwrap();
                saw_sleep |= events
                    .instances
                    .iter()
                    .any(|i| i.change.flags & flags::SLEEP != 0);
                saw_wake |= events
                    .instances
                    .iter()
                    .any(|i| i.change.flags & flags::WAKE != 0);
                assert_eq!(expected.scene.instances.len(), actual.scene.instances.len());
                for a in &actual.scene.instances {
                    let b = expected
                        .scene
                        .instances
                        .iter()
                        .find(|b| b.entity == a.entity)
                        .unwrap();
                    assert_eq!(a.world, b.world);
                }
            }
            if id == "body-sleep-wake" {
                assert!(saw_sleep && saw_wake);
            }
        }
    }
}
