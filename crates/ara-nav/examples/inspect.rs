//! Exports a real navmesh, path and counterflow crowd trace for visual verification.
use ara_nav::*;
use bevy_ecs::{prelude::World, schedule::Schedule};
use serde::Serialize;

#[derive(Serialize)]
struct Capture {
    mesh: Vec<DebugPrimitive>,
    path: NavPath,
    frames: Vec<Vec<(u64, [f32; 3])>>,
}
fn floor(g: &mut Geometry, min: [f32; 2], max: [f32; 2]) {
    let i = g.vertices.len() as u32;
    g.vertices.extend([
        [min[0], 0.0, min[1]],
        [max[0], 0.0, min[1]],
        [max[0], 0.0, max[1]],
        [min[0], 0.0, max[1]],
    ]);
    g.triangles.extend([[i, i + 2, i + 1], [i, i + 3, i + 2]]);
    g.areas.extend([0; 2]);
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .ok_or("usage: cargo run -p ara-nav --example inspect -- capture.json")?;
    let mut geometry = Geometry::default();
    floor(&mut geometry, [0.0, 0.0], [24.0, 24.0]);
    floor(&mut geometry, [28.0, 4.0], [36.0, 20.0]);
    let settings = BakeSettings {
        tile_size: 32,
        ..Default::default()
    };
    let tiles = BakeInput::tiles_in(
        &settings,
        Bounds {
            min: [0.0, -1.0, 0.0],
            max: [36.0, 2.0, 24.0],
        },
    )?;
    let input = BakeInput {
        settings,
        geometry,
        tiles,
        links: vec![OffMeshLink {
            id: 7,
            start: [23.0, 0.1, 12.0],
            end: [29.0, 0.1, 12.0],
            radius: 0.7,
            area: 1,
            bidirectional: true,
            enabled: true,
            ..OffMeshLink::default()
        }],
        obstacles: vec![Obstacle {
            id: 1,
            bounds: Bounds {
                min: [10.0, -1.0, 6.0],
                max: [14.0, 3.0, 18.0],
            },
            area: Obstacle::CARVE,
        }],
    };
    let artifact = Baker::new(None).bake(&input, None)?;
    let mesh = NavMesh::from_artifact(&artifact)?;
    let mut debug = Vec::new();
    mesh.debug_stream(|p| debug.push(p));
    let path = mesh
        .query()?
        .path([3.0, 0.1, 12.0], [33.0, 0.1, 12.0], &Default::default())?;
    assert!(path.complete && path.corners.iter().any(|c| c.off_mesh));
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, Default::default())?;
    world.resource_mut::<NavWorld>().stage(mesh);
    for id in 0..40 {
        let x = if id < 20 { 3.0 } else { 21.0 };
        let z = 4.0 + (id % 20) as f32 * 0.75;
        let entity = world
            .spawn((
                NavAgent::default(),
                NavPosition([x, 0.1, z]),
                NavAgentState::new(id),
            ))
            .id();
        goto(&mut world, entity, GotoTarget::Point([24.0 - x, 0.1, z]))?;
    }
    let mut frames = Vec::new();
    let mut reached = std::collections::BTreeSet::new();
    for tick in 0..1800 {
        schedule.run(&mut world);
        reached.extend(
            world
                .query::<&NavAgentState>()
                .iter(&world)
                .filter(|s| s.status == NavStatus::Arrived)
                .map(|s| s.id),
        );
        if tick % 30 == 0 {
            let mut frame: Vec<_> = world
                .query::<(&NavPosition, &NavAgentState)>()
                .iter(&world)
                .map(|(p, s)| (s.id, p.0))
                .collect();
            frame.sort_by_key(|&(id, _)| id);
            frames.push(frame);
        }
    }
    assert!(
        world.resource::<NavWorld>().errors().is_empty(),
        "{:?}",
        world.resource::<NavWorld>().errors()
    );
    let arrived = world
        .query::<&NavAgentState>()
        .iter(&world)
        .filter(|s| s.status == NavStatus::Arrived)
        .count();
    assert_eq!(
        reached.len(),
        40,
        "every counterflow agent must reach its target"
    );
    let capture = Capture {
        mesh: debug,
        path,
        frames,
    };
    std::fs::write(
        output,
        ara_json::to_canonical_string(&ara_json::to_value(&capture)?),
    )?;
    println!(
        "40 counterflow agents, 1800 fixed ticks, {} reached, {arrived} currently at goal; mesh and trace exported",
        reached.len()
    );
    Ok(())
}
