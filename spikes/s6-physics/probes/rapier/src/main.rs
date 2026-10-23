//! Minimal Rapier user for binary size / clean build time: a box falls onto the ground.
use rapier3d::prelude::*;

fn main() {
    let mut world = PhysicsWorld::new();
    world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(0.0, -0.5, 0.0)),
        ColliderBuilder::cuboid(10.0, 0.5, 10.0),
    );
    let (cube, _) = world.insert(
        RigidBodyBuilder::dynamic().translation(Vector::new(0.0, 5.0, 0.0)),
        ColliderBuilder::cuboid(0.5, 0.5, 0.5),
    );
    for _ in 0..60 {
        world.step();
    }
    println!("{}", world.bodies[cube].translation().y);
}
