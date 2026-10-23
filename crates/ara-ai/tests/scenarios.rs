#![allow(clippy::unwrap_used)]
//! Golden behaviour scenarios on a baked map, run headless at 60 Hz.
mod common;
use ara_ai::*;
use ara_nav::Geometry;
use ara_perception::Perceiver;
use common::*;

/// Patrol -> investigate a noise -> chase the player -> lose them -> return.
#[test]
fn guard_patrols_investigates_chases_loses_and_returns() {
    // 60 x 40 yard split by a wall (x 28..30) open only at the north end.
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 60.0], [0.0, 40.0], 0.0, 0);
    solid(&mut g, [28.0, 30.0], [0.0, 32.0]);
    let mut sim = Sim::new(bake(g, [60.0, 40.0]));
    sim.load("guard", "guard");
    let senses = Perceiver {
        sight_range: 20.0,
        ..Default::default()
    };
    let guard = sim.npc(
        1,
        "guard",
        [6.0, 0.1, 6.0],
        senses,
        &["route = [vec3(6, 0, 6), vec3(6, 0, 34), vec3(14, 0, 34), vec3(14, 0, 6)]"],
    );
    let player = sim.actor(99, "player", [55.0, 0.1, 37.0]);
    let mut fleeing = false;
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..5400 {
        if sim.tick == 180 {
            sim.noise([17.0, 0.0, 20.0], 30.0);
        }
        // From 16 s the player walks toward the guard until spotted, then sprints
        // east around the wall's north end and south behind it, out of range.
        let at = sim.position(player);
        if sim.leaf(guard) == "Chase" {
            fleeing = true;
        }
        if fleeing {
            let target = if at[2] > 30.0 && at[0] < 40.0 {
                [42.0, 0.1, 37.0]
            } else {
                [55.0, 0.1, 4.0]
            };
            sim.move_actor(player, toward(at, target, 6.5 * DT));
        } else if sim.tick >= 960 {
            let goal = sim.position(guard);
            sim.move_actor(player, toward(at, [goal[0], 0.1, goal[2]], 2.0 * DT));
        }
        sim.step();
        let leaf = sim.leaf(guard);
        if seen.last() != Some(&leaf) {
            seen.push(leaf);
        }
    }
    for line in &sim.log {
        println!("{line}");
    }
    let order = [
        "Patrol",
        "GoTo",
        "LookAround",
        "Patrol",
        "Chase",
        "Search",
        "Patrol",
    ];
    let mut at = 0;
    for state in &seen {
        if at < order.len() && *state == order[at] {
            at += 1;
        }
    }
    assert_eq!(at, order.len(), "state sequence {seen:?}");
    // Returned to the patrol route on the west side.
    let p = sim.world.get::<ara_nav::NavPosition>(guard).unwrap().0;
    assert!(p[0] < 20.0, "guard at {p:?}");
    golden("guard", &sim.log);
}

/// Cover fight: engage while healthy, hide from the enemy when hurt, recover, re-engage.
#[test]
fn soldier_takes_hidden_cover_when_hurt_and_reengages() {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 40.0], [0.0, 40.0], 0.0, 0);
    for (x, z) in [(18.0, 10.0), (18.0, 28.0), (12.0, 15.0), (12.0, 23.0)] {
        solid(&mut g, [x, x + 2.0], [z, z + 2.0]);
    }
    let mesh = bake(g, [40.0, 40.0]);
    let los = NavLineOfSight::new(mesh.clone());
    let mut sim = Sim::new(mesh);
    sim.load("soldier", "soldier");
    let senses = Perceiver {
        sight_range: 40.0,
        ..Default::default()
    };
    let soldier = sim.npc(1, "soldier", [6.0, 0.1, 20.0], senses, &[]);
    let enemy = sim.actor(99, "player", [34.0, 0.1, 20.0]);
    // Start facing the enemy (spawn orientation is the host's job).
    sim.world
        .get_mut::<ara_perception::PerceptionPose>(soldier)
        .unwrap()
        .forward = [1.0, 0.0, 0.0];
    let eye = |p: [f32; 3]| [p[0], p[1] + EYE, p[2]];
    let mut shots = 0u32;
    let mut sequence = 0;
    let mut healed_in_cover = false;
    let mut cover_hidden = true;
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..3600 {
        sim.step();
        let p = sim.position(soldier);
        let exposed =
            ara_perception::LineOfSight::clear(&los, eye(sim.position(enemy)), eye(p), 99, 1);
        let health = sim.var(soldier, "health").float();
        let leaf = sim.leaf(soldier);
        let next = (health + if exposed { -12.0 } else { 6.0 } * DT).clamp(0.0, 100.0);
        if leaf == "Hold" && next > health {
            healed_in_cover = true;
        }
        sim.set_var(soldier, "health", Value::Float(next));
        shots = shots.max(sim.world.get::<ActionIntent>(soldier).unwrap().sequence);
        if leaf == "GoToCover"
            && let Value::Vec3(c) = sim.var(soldier, "cover")
        {
            cover_hidden &=
                !ara_perception::LineOfSight::clear(&los, eye(sim.position(enemy)), eye(c), 99, 1);
        }
        if seen.last() != Some(&leaf) {
            seen.push(leaf.clone());
            if (sequence % 2 == 0 && leaf == "Shoot") || (sequence % 2 == 1 && leaf == "Hold") {
                sequence += 1;
            }
        }
    }
    for line in &sim.log {
        println!("{line}");
    }
    println!("{seen:?} shots={shots}");
    assert!(
        sequence >= 3,
        "expected shoot -> hold -> shoot, got {seen:?}"
    );
    assert!(shots > 4, "fired {shots} shots");
    assert!(healed_in_cover);
    assert!(cover_hidden, "chosen cover was visible to the enemy");
    golden("cover", &sim.log);
}
