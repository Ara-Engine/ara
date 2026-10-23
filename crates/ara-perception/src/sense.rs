//! One fixed perception step: sight cones, hearing and memory decay.
use crate::{
    Known, LineOfSight, NoiseEvents, OpenSpace, Perceiver, Perception, PerceptionPose, Sense,
    SightOutcome, Stimulus, kind_hash,
};
use bevy_ecs::{
    prelude::{Resource, World},
    query::QueryState,
};
use rayon::prelude::*;

/// Fixed perception step (set once; never derived from the wall clock).
#[derive(Debug, Clone, Copy, Resource)]
pub struct PerceptionConfig {
    /// Seconds per fixed tick.
    pub fixed_dt: f32,
}
impl Default for PerceptionConfig {
    fn default() -> Self {
        Self {
            fixed_dt: 1.0 / 60.0,
        }
    }
}

/// Line-of-sight provider used by [`tick`]; absent means open space. Hosts with
/// physics usually call [`sense`] directly with a `PhysicsLineOfSight` instead.
#[derive(Resource)]
pub struct PerceptionLos(pub Box<dyn LineOfSight + Send + Sync>);

struct Target {
    id: u64,
    kind: u32,
    feet: [f32; 3],
    aim: [f32; 3],
    visibility: f32,
}

type PerceiverQuery = QueryState<(
    &'static Perceiver,
    &'static PerceptionPose,
    &'static mut Perception,
)>;

/// ECS system: [`sense`] with the [`PerceptionLos`] resource (or open space).
pub fn tick(world: &mut World) {
    match world.remove_resource::<PerceptionLos>() {
        Some(los) => {
            sense(world, los.0.as_ref());
            world.insert_resource(los);
        }
        None => sense(world, &OpenSpace),
    }
}

/// Updates every `Perception`: sight checks against all `Stimulus` entities,
/// this tick's noises (then clears them) and memory decay. Perceivers run in
/// parallel; each reads only immutable inputs, so results are deterministic.
pub fn sense(world: &mut World, los: &dyn LineOfSight) {
    let dt = world
        .get_resource::<PerceptionConfig>()
        .copied()
        .unwrap_or_default()
        .fixed_dt;
    let noises = world
        .get_resource_mut::<NoiseEvents>()
        .map(|mut n| std::mem::take(&mut n.events))
        .unwrap_or_default();
    let mut targets: Vec<Target> = world
        .query::<(&Stimulus, &PerceptionPose)>()
        .iter(world)
        .filter(|(_, p)| p.position.iter().all(|v| v.is_finite()))
        .map(|(s, p)| Target {
            id: p.id,
            kind: kind_hash(&s.kind),
            feet: p.position,
            aim: [p.position[0], p.position[1] + s.height, p.position[2]],
            visibility: s.visibility.max(0.0),
        })
        .collect();
    targets.sort_by_key(|t| t.id);
    let mut query: PerceiverQuery = world.query();
    let Ok(tables) = query.contiguous_iter_mut(world) else {
        return;
    };
    for (perceivers, poses, mut memories) in tables {
        memories.mark_all_as_changed();
        perceivers
            .par_iter()
            .zip(poses.par_iter())
            .zip(memories.bypass_change_detection().par_iter_mut())
            .with_min_len(64)
            .for_each(|((perceiver, pose), memory)| {
                update(perceiver, pose, memory, &targets, &noises, los, dt);
            });
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn update(
    perceiver: &Perceiver,
    pose: &PerceptionPose,
    memory: &mut Perception,
    targets: &[Target],
    noises: &[crate::Noise],
    los: &dyn LineOfSight,
    dt: f32,
) {
    let eye = [
        pose.position[0],
        pose.position[1] + perceiver.eye_height,
        pose.position[2],
    ];
    let (fx, fz) = (pose.forward[0], pose.forward[2]);
    let flen = (fx * fx + fz * fz).sqrt();
    let (fx, fz) = if flen > 1e-6 {
        (fx / flen, fz / flen)
    } else {
        (0.0, 1.0)
    };
    let cos_half = (perceiver.fov.clamp(0.0, 360.0).to_radians() * 0.5).cos();
    let mut sensed: Vec<(u64, u32, [f32; 3], Sense)> = Vec::new();
    memory.checks.clear();
    for t in targets.iter().filter(|t| t.id != pose.id) {
        let d = sub(t.aim, eye);
        let dist = length(d);
        let range = perceiver.sight_range * t.visibility;
        let peripheral = perceiver.peripheral_range * t.visibility;
        if dist > (2.0 * range).max(peripheral) {
            continue;
        }
        let outcome = if dist > range.max(peripheral) {
            SightOutcome::OutOfRange
        } else {
            let horizontal = (d[0] * d[0] + d[2] * d[2]).sqrt();
            let in_cone = perceiver.fov >= 360.0
                || dist <= peripheral
                || (dist <= range
                    && (horizontal <= 1e-6 || (d[0] * fx + d[2] * fz) / horizontal >= cos_half));
            if !in_cone {
                SightOutcome::OutsideCone
            } else if los.clear(eye, t.aim, pose.id, t.id) {
                SightOutcome::Seen
            } else {
                SightOutcome::Occluded
            }
        };
        memory.checks.push((t.id, outcome));
        if outcome == SightOutcome::Seen {
            sensed.push((t.id, t.kind, t.feet, Sense::Sight));
        }
    }
    for n in noises
        .iter()
        .filter(|n| n.source != pose.id || n.source == 0)
    {
        let dist = length(sub(n.position, eye));
        let radius = n.radius * perceiver.hearing;
        if dist <= radius
            && (dist <= radius * perceiver.occlusion
                || los.clear(eye, n.position, pose.id, n.source))
        {
            sensed.push((n.source, n.kind, n.position, Sense::Hearing));
        }
    }
    for k in &mut memory.known {
        k.since = k.since.saturating_add(1);
        k.visible = false;
        k.heard = false;
    }
    for (id, kind, position, sense) in sensed {
        let slot = match memory.known.binary_search_by_key(&id, |k| k.id) {
            Ok(i) => i,
            Err(i) => {
                memory.known.insert(
                    i,
                    Known {
                        id,
                        kind,
                        position,
                        sense,
                        visible: false,
                        heard: false,
                        since: 0,
                        strength: 1.0,
                    },
                );
                i
            }
        };
        let k = &mut memory.known[slot];
        // Sight is more precise than hearing: a heard noise never overwrites a
        // position seen this tick.
        if !(k.visible && sense == Sense::Hearing) {
            k.position = position;
            k.sense = sense;
        }
        k.kind = kind;
        k.since = 0;
        k.visible |= sense == Sense::Sight;
        k.heard |= sense == Sense::Hearing;
    }
    let lifetime = perceiver.memory.max(dt);
    memory.known.retain_mut(|k| {
        let age = k.since as f32 * dt;
        k.strength = (1.0 - age / lifetime).max(0.0);
        age <= lifetime
    });
}
