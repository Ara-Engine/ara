use ara_physics::{
    BodyDesc, Collider, PhysicsBackend, PhysicsConfig, PhysicsPose, Query, QueryFilter,
    QueryResults, RigidBody, Velocity,
};
use ara_physics_jolt::JoltBackend;
use ara_terrain::{Header, Tile, TileCoord};

#[test]
fn native_heightfields_and_cell_holes_match_terrain_coordinates()
-> Result<(), Box<dyn std::error::Error>> {
    let header = Header {
        origin: [100., 200.],
        spacing: 1.,
        cells: 16,
        min: TileCoord { x: 0, z: 0 },
        max: TileCoord { x: 2, z: 1 },
        materials: vec![],
    };
    let mut backend = JoltBackend::new(PhysicsConfig {
        threads: 1,
        ..PhysicsConfig::default()
    })?;
    let mut tile = Tile::flat(16).map_err(|d| d.message)?;
    tile.heights.fill(3.);
    let mut bodies = Vec::new();
    for x in 0..2 {
        let shape = tile
            .cook_collider(&header, TileCoord { x, z: 0 }, &mut backend)?
            .ok_or("solid tile missing shape")?;
        backend.create_bodies(
            &[BodyDesc {
                shape,
                user_data: x as u64 + 1,
                pose: PhysicsPose::default(),
                body: RigidBody::default(),
                collider: Collider::default(),
                velocity: Velocity::default(),
            }],
            &mut bodies,
        )?;
    }
    let queries: Vec<_> = [115.9, 116.1]
        .map(|x| Query::Ray {
            origin: [x, 10., 208.4],
            displacement: [0., -20., 0.],
            filter: QueryFilter::default(),
        })
        .into_iter()
        .collect();
    let mut hits = QueryResults::default();
    backend.queries(&queries, &mut hits)?;
    for i in 0..2 {
        let hit = hits.get(i).first().ok_or("heightfield seam ray missed")?;
        assert!((hit.fraction - 0.35).abs() < 0.001);
    }

    let mut backend = JoltBackend::new(PhysicsConfig {
        threads: 1,
        ..PhysicsConfig::default()
    })?;
    tile.holes[8 * 17 + 8] = 1;
    let shape = tile
        .cook_collider(&header, TileCoord { x: 0, z: 0 }, &mut backend)?
        .ok_or("partly holed tile missing shape")?;
    backend.create_bodies(
        &[BodyDesc {
            shape,
            user_data: 1,
            pose: PhysicsPose::default(),
            body: RigidBody::default(),
            collider: Collider::default(),
            velocity: Velocity::default(),
        }],
        &mut bodies,
    )?;
    backend.queries(
        &[
            Query::Ray {
                origin: [108.4, 10., 208.3],
                displacement: [0., -20., 0.],
                filter: QueryFilter::default(),
            },
            Query::Ray {
                origin: [107.4, 10., 208.3],
                displacement: [0., -20., 0.],
                filter: QueryFilter::default(),
            },
        ],
        &mut hits,
    )?;
    assert!(hits.get(0).is_empty());
    assert!(!hits.get(1).is_empty());
    tile.holes.fill(1);
    assert!(
        tile.cook_collider(&header, TileCoord { x: 0, z: 0 }, &mut backend)?
            .is_none()
    );
    Ok(())
}
