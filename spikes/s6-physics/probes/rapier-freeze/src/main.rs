//! Repro: in rapier3d 0.36.1, calling `PhysicsWorld::detect_collisions` before the first
//! `step` leaves dynamic bodies frozen forever; calling it after any step is fine.
//! Expected output: the box falls in runs `None` and `Some(5)`, but stays at y=5 in `Some(0)`.
use rapier3d::prelude::*;

fn run(detect_at: Option<usize>) {
    let mut world = PhysicsWorld::new();
    world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(0.0, -0.5, 0.0)),
        ColliderBuilder::cuboid(10.0, 0.5, 10.0),
    );
    let (b, _) = world.insert(
        RigidBodyBuilder::dynamic().translation(Vector::new(0.0, 5.0, 0.0)),
        ColliderBuilder::cuboid(0.5, 0.5, 0.5),
    );
    for i in 0..120 {
        if Some(i) == detect_at {
            world.detect_collisions(&(), &());
        }
        world.step();
    }
    println!(
        "detect_collisions at {detect_at:?}: box y after 2 s = {:.4}",
        world.bodies[b].translation().y
    );
}

fn main() {
    run(None);
    run(Some(0));
    run(Some(5));
}
