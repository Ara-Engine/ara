#![allow(clippy::unwrap_used)]
//! Large worlds: per-cell bakes, hierarchical routes and streamed resident meshes.
mod common;
use ara_nav::*;
use bevy_ecs::{prelude::*, schedule::Schedule};
use common::*;
use std::collections::BTreeSet;

/// Floor of `size` metres with `rows` serpentine walls (gaps alternate sides).
fn maze(size: f32, rows: usize) -> Geometry {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, size], [0.0, size], 0.0, 0);
    let spacing = size / (rows + 1) as f32;
    for row in 1..=rows {
        let z = row as f32 * spacing;
        let (x0, x1) = if row % 2 == 1 {
            (0.0, size - 16.0)
        } else {
            (16.0, size)
        };
        wall(&mut g, [x0, z - 1.0], [x1, z + 1.0]);
    }
    g
}

/// Solid 3 m box (non-walkable faces).
fn wall(g: &mut Geometry, min: [f32; 2], max: [f32; 2]) {
    let (a, b) = (min, max);
    floor(g, [a[0], b[0]], [a[1], b[1]], 3.0, 255);
    for (p, q) in [
        ([a[0], a[1]], [b[0], a[1]]),
        ([b[0], a[1]], [b[0], b[1]]),
        ([b[0], b[1]], [a[0], b[1]]),
        ([a[0], b[1]], [a[0], a[1]]),
    ] {
        quad(
            g,
            [
                [p[0], -0.5, p[1]],
                [q[0], -0.5, q[1]],
                [q[0], 3.0, q[1]],
                [p[0], 3.0, p[1]],
            ],
            255,
        );
    }
}

fn grid(tile: u16, tiles_per_cell: u16) -> CellGrid {
    CellGrid::new(
        BakeSettings {
            origin: [0.0, -2.0, 0.0],
            max_y: 6.0,
            cell_size: 0.5,
            cell_height: 0.2,
            tile_size: tile,
            ..Default::default()
        },
        tiles_per_cell,
    )
    .unwrap()
}

fn cells(grid: &CellGrid, size: f32) -> BTreeSet<CellCoord> {
    grid.cells_in(Bounds {
        min: [0.1, 0.0, 0.1],
        max: [size - 0.1, 1.0, size - 0.1],
    })
}

fn bake_all(grid: &CellGrid, geometry: &Geometry, size: f32) -> Vec<CellBake> {
    let baker = Baker::new(None);
    cells(grid, size)
        .into_iter()
        .map(|c| {
            let source = CellSource {
                geometry: geometry.clone(),
                ..Default::default()
            };
            bake_cell(&baker, grid, c, source, None, 8.0).unwrap()
        })
        .collect()
}

fn length(path: &NavPath) -> f32 {
    path.corners
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0].position, w[1].position);
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        })
        .sum()
}

#[test]
fn cell_bakes_equal_whole_world_tiles_and_route_tracks_full_path() {
    let size = 256.0;
    let grid = grid(64, 2); // 32 m tiles, 64 m cells: 4 x 4 cells.
    let geometry = maze(size, 3);
    let bakes = bake_all(&grid, &geometry, size);
    assert_eq!(bakes.len(), 16);
    // Seams: per-cell bakes with halos produce exactly the whole-world tiles.
    let whole = Baker::new(None)
        .bake(
            &BakeInput {
                settings: grid.settings.clone(),
                geometry: geometry.clone(),
                links: vec![],
                obstacles: vec![],
                tiles: cells(&grid, size)
                    .iter()
                    .flat_map(|c| grid.tiles(*c))
                    .collect(),
            },
            None,
        )
        .unwrap();
    let merged = merge_cells(&grid, &bakes);
    assert_eq!(merged.tiles, whole.tiles);
    assert_eq!(merged.content_hash(), whole.content_hash());

    let streamer = NavStreamer::new(grid.clone(), bakes).unwrap();
    let graph = streamer.graph();
    assert!(graph.portal_count() > 16);
    let (start, goal) = ([8.0, 0.1, 8.0], [8.0, 0.1, 248.0]);
    let mut scratch = RouteScratch::default();
    let (route, stats) = graph.route(start, goal, &mut scratch).unwrap();
    let route = route.unwrap();
    assert!(stats.expanded > 0);
    let mesh = NavMesh::from_artifact(&whole).unwrap();
    let mut query = mesh.query().unwrap();
    query.set_capacity(32768).unwrap();
    let full = query.path(start, goal, &QueryFilter::default()).unwrap();
    assert!(full.complete);
    let exact = length(&full);
    assert!(
        exact > 3.0 * 240.0,
        "maze must force a winding path: {exact}"
    );
    let ratio = route.cost / exact;
    assert!(
        (0.99..1.35).contains(&ratio),
        "route {} vs exact {exact}",
        route.cost
    );
    // Consecutive waypoints are in the same or adjacent cells.
    for pair in route.waypoints.windows(2) {
        let (a, b) = (grid.cell_of(pair[0]), grid.cell_of(pair[1]));
        assert!(
            (a.x - b.x).abs() <= 1 && (a.z - b.z).abs() <= 1,
            "{a:?} -> {b:?}"
        );
    }
    // Goals in cells without a summary are explicit errors, not empty routes.
    assert!(graph.route(start, [900.0, 0.1, 8.0], &mut scratch).is_err());
}

#[test]
fn streamed_agent_follows_route_across_resident_swaps() {
    let size = 192.0;
    let grid = grid(64, 1); // 32 m cells: 6 x 6.
    let geometry = maze(size, 2);
    let mut streamer = NavStreamer::new(grid.clone(), bake_all(&grid, &geometry, size)).unwrap();
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, NavConfig::default()).unwrap();
    world
        .resource_mut::<NavWorld>()
        .set_cluster_graph(0, streamer.graph());
    let start = [8.0, 0.1, 8.0];
    let goal = [8.0, 0.1, 184.0];
    let actor = world
        .spawn((
            NavAgent {
                max_speed: 8.0,
                acceleration: 40.0,
                ..NavAgent::default()
            },
            NavPosition(start),
            NavAgentState::new(1),
        ))
        .id();
    let window = |p: [f32; 3]| {
        let c = grid.cell_of(p);
        (-1..=1)
            .flat_map(|dx| {
                (-1..=1).map(move |dz| CellCoord {
                    x: c.x + dx,
                    z: c.z + dz,
                })
            })
            .collect::<BTreeSet<_>>()
    };
    streamer.request(window(start));
    let mesh = streamer.wait().unwrap().unwrap();
    world.resource_mut::<NavWorld>().stage(mesh);
    goto(&mut world, actor, GotoTarget::Point(goal)).unwrap();
    let mut swaps = 0;
    let mut routed = false;
    for _ in 0..6000 {
        let p = world.get::<NavPosition>(actor).unwrap().0;
        let wanted = window(p);
        if &wanted != streamer.resident() {
            streamer.request(wanted);
            if let Some(mesh) = streamer.wait() {
                world.resource_mut::<NavWorld>().stage(mesh.unwrap());
                swaps += 1;
            }
        }
        schedule.run(&mut world);
        let state = world.get::<NavAgentState>(actor).unwrap();
        routed |= state.route.is_some();
        assert_ne!(state.status, NavStatus::Unreachable, "{:?}", state.issue);
        if state.status == NavStatus::Arrived {
            break;
        }
    }
    let state = world.get::<NavAgentState>(actor).unwrap();
    assert_eq!(
        state.status,
        NavStatus::Arrived,
        "at {:?}",
        world.get::<NavPosition>(actor)
    );
    assert!(routed, "goal starts outside the resident mesh");
    assert!(swaps >= 6, "only {swaps} resident swaps");
}

#[test]
fn heightfield_terrain_bakes_with_holes() {
    // 64 m terrain at 1 m spacing with a gentle wave and a 10 m hole band.
    let cells = 64u32;
    let side = (cells + 1) as usize;
    let mut heights = vec![0.0f32; side * side];
    let mut holes = vec![0u8; side * side];
    for z in 0..side {
        for x in 0..side {
            heights[z * side + x] = (x as f32 * 0.15).sin() * 0.8;
            if (30..40).contains(&z) && x < 54 {
                holes[z * side + x] = 1;
            }
        }
    }
    let mut g = Geometry::default();
    g.append_heightfield([0.0, 0.0, 0.0], 1.0, cells, &heights, &holes, 0)
        .unwrap();
    assert!(
        g.append_heightfield([0.0; 3], 1.0, cells, &heights[1..], &[], 0)
            .is_err()
    );
    let (_, mesh) = bake(&input(
        g,
        Bounds {
            min: [0.0, -2.0, 0.0],
            max: [64.0, 2.0, 64.0],
        },
    ));
    let mut query = mesh.query().unwrap();
    let path = query
        .path([5.0, 0.5, 5.0], [5.0, 0.5, 58.0], &QueryFilter::default())
        .unwrap();
    assert!(path.complete);
    // The hole band forces a detour around its open end near x = 54..64.
    assert!(
        path.corners.iter().any(|c| c.position[0] > 50.0),
        "{:?}",
        path.corners
    );
}
