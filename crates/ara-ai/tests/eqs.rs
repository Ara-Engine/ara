#![allow(clippy::unwrap_used)]
//! EQS queries for cover, flanking and wander, evaluated by a running brain.
mod common;
use ara_ai::*;
use ara_nav::{Geometry, NavPosition};
use ara_perception::{LineOfSight, Perceiver, PerceptionPose};
use common::*;

const TACTICS: &str = r#"behavior Tactics {
  var cover: vec3;
  var flank: vec3;

  query Cover {
    generate cover(self, 16);
    filter navigable(item);
    filter not visible(lastSeen(nearest("player")), item);
    score inverse(distance(item, self), 16);
  }

  query Flank {
    generate ring(lastSeen(nearest("player")), 10, 24);
    filter navigable(item);
    score 1 - abs(dot(direction(lastSeen(nearest("player")), item), direction(lastSeen(nearest("player")), self)));
    score inverse(distance(item, self), 30) weight 0.25;
  }

  state Plan {
    do find(Cover, into: cover);
    do find(Flank, into: flank);
    on done -> Roam;
  }
  state Roam {
    do wander(6);
  }
}"#;

#[test]
fn cover_is_hidden_flank_is_perpendicular_and_wander_stays_near_home() {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 40.0], [0.0, 40.0], 0.0, 0);
    for (x, z) in [(18.0, 10.0), (18.0, 28.0), (12.0, 15.0), (12.0, 23.0)] {
        solid(&mut g, [x, x + 2.0], [z, z + 2.0]);
    }
    let mesh = bake(g, [40.0, 40.0]);
    let los = NavLineOfSight::new(mesh.clone());
    let mut sim = Sim::new(mesh);
    sim.world
        .resource_mut::<AiWorld>()
        .load("tactics", TACTICS)
        .unwrap();
    let npc = sim.npc(
        1,
        "tactics",
        [6.0, 0.1, 20.0],
        Perceiver {
            sight_range: 40.0,
            ..Default::default()
        },
        &[],
    );
    sim.world.get_mut::<PerceptionPose>(npc).unwrap().forward = [1.0, 0.0, 0.0];
    let enemy = [26.0, 0.1, 20.0];
    sim.actor(99, "player", enemy);
    sim.step();
    let eye = |p: [f32; 3]| [p[0], p[1] + EYE, p[2]];
    let Value::Vec3(cover) = sim.var(npc, "cover") else {
        panic!("no cover found")
    };
    assert!(
        !los.clear(eye(enemy), eye(cover), 99, 1),
        "cover {cover:?} is visible"
    );
    let Value::Vec3(flank) = sim.var(npc, "flank") else {
        panic!("no flank found")
    };
    let a = [flank[0] - enemy[0], flank[2] - enemy[2]];
    let b = [6.0 - enemy[0], 20.0 - enemy[2]];
    let cos = (a[0] * b[0] + a[1] * b[1])
        / ((a[0] * a[0] + a[1] * a[1]).sqrt() * (b[0] * b[0] + b[1] * b[1]).sqrt());
    assert!(
        cos.abs() < 0.3,
        "flank {flank:?} is not to the side (cos {cos})"
    );
    // `on done` is checked at the start of the next tick.
    sim.step();
    assert_eq!(sim.leaf(npc), "Roam");
    let mut farthest: f32 = 0.0;
    for _ in 0..900 {
        sim.step();
        let p = sim.world.get::<NavPosition>(npc).unwrap().0;
        farthest = farthest.max(((p[0] - 6.0).powi(2) + (p[2] - 20.0).powi(2)).sqrt());
    }
    assert!(farthest > 1.0, "wander never moved");
    assert!(farthest < 7.5, "wander left its radius: {farthest}");
}
