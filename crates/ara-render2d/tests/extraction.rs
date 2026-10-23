use ara_2d::{Sprite, TileLayer, Tilemap};
use ara_components::Transform;
use ara_render2d::Scene2d;
use ara_sim::{Identity, bevy_ecs::prelude::*};
#[test]
fn extraction_composes_hierarchy_basis_and_uses_absolute_physics_pose() -> anyhow::Result<()> {
    let mut world = World::new();
    let parent = world
        .spawn((
            Identity("e_parent".into()),
            Transform {
                position: [3., 4., 0.],
                rotation: [0., 0., 90.],
                scale: [2., 3., 1.],
            },
        ))
        .id();
    world.spawn((
        Identity("e_sprite".into()),
        ChildOf(parent),
        Transform {
            position: [1., 0., 0.],
            ..Default::default()
        },
        Sprite::default(),
    ));
    world.spawn((
        Identity("e_physics".into()),
        ChildOf(parent),
        ara_physics2d::PhysicsPose2d {
            position: [9., 8.],
            rotation: 0.,
        },
        Sprite::default(),
    ));
    let scene = Scene2d::extract(&mut world, Default::default(), 0)?;
    let child = scene
        .sprites
        .iter()
        .find(|s| s.name == "e_sprite")
        .expect("extracted sprite");
    assert!((child.position[0] - 3.).abs() < 0.00001 && (child.position[1] - 6.).abs() < 0.00001);
    assert!((child.basis[1] + 3.).abs() < 0.00001 && (child.basis[2] - 2.).abs() < 0.00001);
    let physics = scene
        .sprites
        .iter()
        .find(|s| s.name == "e_physics")
        .expect("extracted body sprite");
    assert_eq!(physics.position, [9., 8.]);
    // Empty layers still participate in a legal source projection.
    world.spawn((
        Identity("e_map".into()),
        Tilemap {
            layers: vec![TileLayer {
                id: "l_ground".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    ));
    assert_eq!(
        Scene2d::extract(&mut world, Default::default(), 0)?
            .tilemaps
            .len(),
        1
    );
    Ok(())
}
