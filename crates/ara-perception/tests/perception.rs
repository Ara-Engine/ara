#![allow(clippy::unwrap_used)]
use ara_perception::*;
use bevy_ecs::prelude::*;

fn perceiver(world: &mut World, id: u64, at: [f32; 3], forward: [f32; 3]) -> Entity {
    world
        .spawn((
            Perceiver::default(),
            PerceptionPose {
                id,
                position: at,
                forward,
            },
            Perception::default(),
        ))
        .id()
}

fn target(world: &mut World, id: u64, at: [f32; 3], visibility: f32) -> Entity {
    world
        .spawn((
            Stimulus {
                kind: "player".into(),
                visibility,
                ..Default::default()
            },
            PerceptionPose {
                id,
                position: at,
                forward: [0.0, 0.0, 1.0],
            },
        ))
        .id()
}

/// A wall on the plane x = 5 (from z = 0 to 100) blocks sight lines crossing it.
fn wall(from: [f32; 3], to: [f32; 3], _: u64, _: u64) -> bool {
    let crosses = (from[0] - 5.0) * (to[0] - 5.0) < 0.0;
    if !crosses {
        return true;
    }
    let t = (5.0 - from[0]) / (to[0] - from[0]);
    let z = from[2] + (to[2] - from[2]) * t;
    !(0.0..100.0).contains(&z)
}

#[test]
fn cone_range_peripheral_occlusion_and_visibility() {
    let mut world = World::new();
    let viewer = perceiver(&mut world, 1, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    target(&mut world, 10, [0.0, 0.0, 10.0], 1.0); // Ahead.
    target(&mut world, 11, [0.0, 0.0, -10.0], 1.0); // Behind.
    target(&mut world, 12, [0.0, 0.0, 40.0], 1.0); // Too far (25 m range).
    target(&mut world, 13, [0.0, 0.0, -1.5], 1.0); // Behind but peripheral.
    target(&mut world, 14, [3.0, 0.0, 12.0], 0.0); // Invisible.
    target(&mut world, 15, [8.0, 0.0, 12.0], 1.0); // Ahead, behind the wall.
    sense(&mut world, &wall);
    let memory = world.get::<Perception>(viewer).unwrap();
    assert_eq!(memory.check(10), Some(SightOutcome::Seen));
    assert_eq!(memory.check(11), Some(SightOutcome::OutsideCone));
    assert_eq!(memory.check(12), Some(SightOutcome::OutOfRange));
    assert_eq!(memory.check(13), Some(SightOutcome::Seen));
    assert_eq!(
        memory.check(14),
        None,
        "invisible targets are not candidates"
    );
    assert_eq!(memory.check(15), Some(SightOutcome::Occluded));
    let seen: Vec<u64> = memory.known.iter().map(|k| k.id).collect();
    assert_eq!(seen, vec![10, 13]);
    assert!(memory.get(10).unwrap().visible);
    assert_eq!(memory.get(10).unwrap().kind, kind_hash("player"));
}

#[test]
fn hearing_respects_radius_and_occlusion_then_memory_decays() {
    let mut world = World::new();
    world.insert_resource(PerceptionConfig { fixed_dt: 0.5 });
    let viewer = perceiver(&mut world, 1, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let mut noises = NoiseEvents::default();
    // Clear path, in range.
    noises.push(Noise {
        source: 20,
        position: [-8.0, 0.0, -8.0],
        radius: 15.0,
        kind: kind_hash("bottle"),
    });
    // Behind the wall: muffled to half radius (10 m < 14.2 m away), not heard.
    noises.push(Noise {
        source: 21,
        position: [10.0, 0.0, 10.0],
        radius: 20.0,
        kind: 0,
    });
    // Behind the wall but close enough to hear through it.
    noises.push(Noise {
        source: 22,
        position: [6.0, 1.7, 0.0],
        radius: 20.0,
        kind: 0,
    });
    world.insert_resource(noises);
    sense(&mut world, &wall);
    assert!(world.resource::<NoiseEvents>().pending().is_empty());
    let memory = world.get::<Perception>(viewer).unwrap().clone();
    let heard: Vec<u64> = memory.known.iter().map(|k| k.id).collect();
    assert_eq!(heard, vec![20, 22]);
    assert_eq!(memory.get(20).unwrap().sense, Sense::Hearing);
    // Memory lasts 8 s: strength decays linearly, then the entry is dropped.
    for step in 1..=16 {
        sense(&mut world, &wall);
        let memory = world.get::<Perception>(viewer).unwrap();
        let k = memory.get(20).unwrap();
        assert_eq!(k.since, step);
        assert!(!k.heard);
        assert!((k.strength - (1.0 - step as f32 * 0.5 / 8.0)).abs() < 1e-6);
    }
    sense(&mut world, &wall);
    assert!(world.get::<Perception>(viewer).unwrap().get(20).is_none());
}

#[test]
fn parallel_updates_are_identical_on_1_4_and_8_workers() {
    let run = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let mut world = World::new();
            let mut ids = Vec::new();
            for i in 0..2000u64 {
                let a = i as f32 * 0.37;
                ids.push(perceiver(
                    &mut world,
                    i + 1,
                    [(i % 50) as f32 * 2.0, 0.0, (i / 50) as f32 * 2.0],
                    [a.sin(), 0.0, a.cos()],
                ));
            }
            for j in 0..50u64 {
                target(
                    &mut world,
                    10_000 + j,
                    [(j * 7 % 100) as f32, 0.0, (j * 13 % 80) as f32],
                    1.0,
                );
            }
            for _ in 0..5 {
                sense(&mut world, &wall);
            }
            let mut out: Vec<(u64, Perception)> = world
                .query::<(&PerceptionPose, &Perception)>()
                .iter(&world)
                .map(|(p, m)| (p.id, m.clone()))
                .collect();
            out.sort_by_key(|(id, _)| *id);
            serde_json::to_string(&out).unwrap()
        })
    };
    let one = run(1);
    assert_eq!(one, run(4));
    assert_eq!(one, run(8));
}
