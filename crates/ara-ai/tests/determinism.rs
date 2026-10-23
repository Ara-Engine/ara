#![allow(clippy::unwrap_used)]
//! Cross-process determinism: the same scenario hashed in child processes with
//! 1, 4 and 8 Rayon workers, and in-process with reversed spawn order.
mod common;
use ara_ai::*;
use ara_nav::{Geometry, NavAgentState, NavPosition};
use ara_perception::{Perceiver, Perception, PerceptionPose};
use common::*;

/// 96 guards and 32 soldiers on a pillared yard with 4 scripted intruders and
/// periodic noises, 900 ticks. Returns a BLAKE3 hash of all simulation state.
fn run(reverse: bool) -> String {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 80.0], [0.0, 80.0], 0.0, 0);
    for k in 0..16 {
        let (x, z) = (10.0 + (k % 4) as f32 * 18.0, 10.0 + (k / 4) as f32 * 18.0);
        solid(&mut g, [x, x + 3.0], [z, z + 3.0]);
    }
    let mut sim = Sim::new(bake(g, [80.0, 80.0]));
    sim.load("guard", "guard");
    sim.load("soldier", "soldier");
    let mut ids: Vec<u64> = (1..=128).collect();
    if reverse {
        ids.reverse();
    }
    for id in ids {
        let (x, z) = (4.0 + (id % 12) as f32 * 6.3, 4.0 + (id / 12) as f32 * 6.5);
        let senses = Perceiver {
            sight_range: 18.0,
            ..Default::default()
        };
        if id <= 96 {
            let route = format!(
                "route = [vec3({x}, 0, {z}), vec3({}, 0, {z}), vec3({}, 0, {})]",
                (x + 5.0).min(78.0),
                (x + 5.0).min(78.0),
                (z + 4.0).min(78.0)
            );
            sim.npc(id, "guard", [x, 0.1, z], senses, &[&route]);
        } else {
            sim.npc(id, "soldier", [x, 0.1, z], senses, &[]);
        }
    }
    let intruders: Vec<_> = (0..4)
        .map(|k| sim.actor(1000 + k, "player", [2.0 + k as f32 * 20.0, 0.1, 78.0]))
        .collect();
    for tick in 0..900u64 {
        if tick % 120 == 60 {
            sim.noise([(tick % 70) as f32 + 5.0, 0.0, 40.0], 25.0);
        }
        for (k, e) in intruders.iter().enumerate() {
            let at = sim.position(*e);
            let goal = if (tick / 300) % 2 == 0 {
                [70.0 - k as f32 * 15.0, 0.1, 6.0]
            } else {
                [6.0 + k as f32 * 15.0, 0.1, 74.0]
            };
            sim.move_actor(*e, toward(at, goal, 3.0 * DT));
        }
        sim.step();
    }
    assert!(
        sim.log.len() > 200,
        "scenario must churn states: {}",
        sim.log.len()
    );
    let mut rows: Vec<_> = sim
        .world
        .query::<(
            &PerceptionPose,
            Option<&NavPosition>,
            Option<&NavAgentState>,
            Option<&BrainState>,
            Option<&Perception>,
        )>()
        .iter(&sim.world)
        .map(|(p, n, s, b, m)| serde_json::to_string(&(p, n, s, b, m)).unwrap())
        .collect();
    rows.sort();
    let mut log = sim.log.clone();
    log.sort();
    blake3::hash((rows.join("\n") + &log.join("\n")).as_bytes())
        .to_hex()
        .to_string()
}

#[test]
fn child_hash() {
    let Ok(workers) = std::env::var("ARA_AI_DET_WORKERS") else {
        return; // Only meaningful when spawned by the parent test.
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers.parse().unwrap())
        .build()
        .unwrap();
    println!("ARA_DET_HASH={}", pool.install(|| run(false)));
}

#[test]
fn identical_state_hashes_across_processes_worker_counts_and_spawn_order() {
    if std::env::var("ARA_AI_DET_WORKERS").is_ok() {
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let hash = |workers: usize| {
        let out = std::process::Command::new(&exe)
            .args([
                "--exact",
                "child_hash",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env("ARA_AI_DET_WORKERS", workers.to_string())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.lines()
            .find_map(|l| l.split_once("ARA_DET_HASH=").map(|(_, h)| h.trim()))
            .unwrap_or_else(|| {
                panic!(
                    "child failed: {text}\n{}",
                    String::from_utf8_lossy(&out.stderr)
                )
            })
            .to_owned()
    };
    let one = hash(1);
    assert_eq!(one, hash(4), "1 vs 4 workers");
    assert_eq!(one, hash(8), "1 vs 8 workers");
    // Entity spawn order (and so ECS iteration order) must not matter either.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    assert_eq!(one, pool.install(|| run(true)), "reversed spawn order");
    println!("determinism hash {one}");
}
