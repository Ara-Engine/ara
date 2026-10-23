//! Scale measurements: 10k-agent navigation ticks and 16 km² hierarchical routes.
//! `cargo bench -p ara-nav --bench scale` (CARGO_BUILD_JOBS=2 on shared hosts).
use ara_nav::*;
use bevy_ecs::{prelude::World, schedule::Schedule};
use rayon::prelude::*;
use std::{collections::BTreeSet, sync::Arc, time::Instant};

fn quad(g: &mut Geometry, x: [f32; 2], z: [f32; 2], y: f32, area: u8) {
    let i = g.vertices.len() as u32;
    g.vertices.extend([
        [x[0], y, z[0]],
        [x[1], y, z[0]],
        [x[1], y, z[1]],
        [x[0], y, z[1]],
    ]);
    g.triangles.extend([[i, i + 2, i + 1], [i, i + 3, i + 2]]);
    g.areas.extend([area; 2]);
}

/// A 3 m solid box: non-walkable top plus vertical faces that block the floor.
fn solid(g: &mut Geometry, x: [f32; 2], z: [f32; 2]) {
    quad(g, x, z, 3.0, 255);
    let side = |g: &mut Geometry, a: [f32; 2], b: [f32; 2]| {
        let i = g.vertices.len() as u32;
        g.vertices.extend([
            [a[0], -0.5, a[1]],
            [b[0], -0.5, b[1]],
            [b[0], 3.0, b[1]],
            [a[0], 3.0, a[1]],
        ]);
        g.triangles.extend([[i, i + 2, i + 1], [i, i + 3, i + 2]]);
        g.areas.extend([255; 2]);
    };
    side(g, [x[0], z[0]], [x[1], z[0]]);
    side(g, [x[1], z[0]], [x[1], z[1]]);
    side(g, [x[1], z[1]], [x[0], z[1]]);
    side(g, [x[0], z[1]], [x[0], z[0]]);
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() - 1) as f64 * p) as usize]
}

/// 10,000 agents in four blocks crossing a 240 m plaza with 16 carved pillars.
fn crowd(
    solver: CrowdSolver,
    threads: usize,
    agents: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut geometry = Geometry::default();
    quad(&mut geometry, [0.0, 240.0], [0.0, 240.0], 0.0, 0);
    let settings = BakeSettings {
        origin: [0.0, -2.0, 0.0],
        max_y: 4.0,
        cell_size: 0.25,
        tile_size: 128,
        ..Default::default()
    };
    let obstacles = (0..16)
        .map(|i| {
            let (x, z) = (60.0 + (i % 4) as f32 * 40.0, 60.0 + (i / 4) as f32 * 40.0);
            Obstacle::carve(
                i,
                Bounds {
                    min: [x - 2.0, -1.0, z - 2.0],
                    max: [x + 2.0, 3.0, z + 2.0],
                },
            )
        })
        .collect();
    let input = BakeInput {
        tiles: BakeInput::tiles_in(
            &settings,
            Bounds {
                min: [0.0, -1.0, 0.0],
                max: [240.0, 1.0, 240.0],
            },
        )?,
        settings,
        geometry,
        links: vec![],
        obstacles,
    };
    let artifact = Baker::new(None).bake(&input, None)?;
    let mesh = NavMesh::from_artifact(&artifact)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(
        &mut world,
        &mut schedule,
        NavConfig {
            crowd: solver,
            ..Default::default()
        },
    )?;
    world.resource_mut::<NavWorld>().stage(mesh);
    let side = (agents as f32 / 4.0).sqrt().ceil() as usize;
    for i in 0..agents {
        let block = i % 4;
        let k = i / 4;
        let (gx, gz) = ((k % side) as f32 * 1.0, (k / side) as f32 * 1.0);
        let (sx, sz) = [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)][block];
        let p = [120.0 - sx * (20.0 + gx), 0.1, 120.0 - sz * (20.0 + gz)];
        let goal = [120.0 + sx * (20.0 + gx), 0.1, 120.0 + sz * (20.0 + gz)];
        let mut state = NavAgentState::new(i as u64);
        state.set_target(Some(goal));
        world.spawn((NavAgent::default(), NavPosition(p), state));
    }
    // Warm up: plan every corridor within the replan budget.
    let warm = agents / NavConfig::default().replan_budget + 20;
    for _ in 0..warm {
        pool.install(|| schedule.run(&mut world));
    }
    let mut samples = Vec::new();
    for _ in 0..300 {
        let start = Instant::now();
        pool.install(|| schedule.run(&mut world));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let errors = world.resource::<NavWorld>().errors().len();
    let moving = world
        .query::<&NavAgentState>()
        .iter(&world)
        .filter(|s| s.status == NavStatus::Moving)
        .count();
    println!(
        "nav_tick: solver={solver:?} agents={agents} workers={threads} p50_ms={:.3} p95_ms={:.3} max_ms={:.3} moving={moving} errors={errors}",
        percentile(&mut samples, 0.5),
        percentile(&mut samples, 0.95),
        percentile(&mut samples, 1.0),
    );
    Ok(())
}

/// 4096 m x 4096 m (16 km²) floor in 256 m cells with two serpentine walls, so
/// the corner-to-corner route is over 10 km.
fn large_world() -> Result<(), Box<dyn std::error::Error>> {
    let size = 4096.0f32;
    let grid = CellGrid::new(
        BakeSettings {
            origin: [0.0, -2.0, 0.0],
            max_y: 6.0,
            cell_size: 1.0,
            cell_height: 0.25,
            tile_size: 64,
            ..Default::default()
        },
        4,
    )?;
    let walls = [(1365.0f32, false), (2730.0, true)];
    let cells: Vec<CellCoord> = grid
        .cells_in(Bounds {
            min: [1.0, 0.0, 1.0],
            max: [size - 1.0, 1.0, size - 1.0],
        })
        .into_iter()
        .collect();
    let halo = 8.0;
    let start = Instant::now();
    let bakes: Result<Vec<CellBake>, NavError> = cells
        .par_iter()
        .map(|&cell| {
            let b = grid.bounds(cell);
            let (x0, x1) = ((b.min[0] - halo).max(0.0), (b.max[0] + halo).min(size));
            let (z0, z1) = ((b.min[2] - halo).max(0.0), (b.max[2] + halo).min(size));
            let mut geometry = Geometry::default();
            quad(&mut geometry, [x0, x1], [z0, z1], 0.0, 0);
            for (z, gap_west) in walls {
                if z + 2.0 < z0 || z - 2.0 > z1 {
                    continue;
                }
                // Wall spans the world except a 32 m gap at one end.
                let (wx0, wx1): (f32, f32) = if gap_west {
                    (32.0, size)
                } else {
                    (0.0, size - 32.0)
                };
                let (wx0, wx1) = (wx0.max(x0), wx1.min(x1));
                if wx0 < wx1 {
                    solid(&mut geometry, [wx0, wx1], [z - 2.0, z + 2.0]);
                }
            }
            let source = CellSource {
                geometry,
                ..Default::default()
            };
            bake_cell(&Baker::new(None), &grid, cell, source, None, 16.0)
        })
        .collect();
    let bakes = bakes?;
    let bake_ms = start.elapsed().as_secs_f64() * 1000.0;
    let tiles: usize = bakes.iter().map(|b| b.artifact.tiles.len()).sum();
    let start = Instant::now();
    let streamer = NavStreamer::new(grid.clone(), bakes)?;
    let graph: Arc<ClusterGraph> = streamer.graph();
    let graph_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "world_16km2: cells={} tiles={tiles} bake_parallel_ms={bake_ms:.0} graph_ms={graph_ms:.1} portals={} edges={}",
        cells.len(),
        graph.portal_count(),
        graph.edge_count()
    );
    let mut scratch = RouteScratch::default();
    let queries = [
        ([40.0, 0.1, 40.0], [40.0, 0.1, 4050.0]),
        ([2000.0, 0.1, 100.0], [100.0, 0.1, 4000.0]),
        ([4000.0, 0.1, 40.0], [3900.0, 0.1, 4050.0]),
    ];
    for (a, b) in queries {
        let mut samples = Vec::new();
        let mut result = None;
        for _ in 0..200 {
            let start = Instant::now();
            let r = graph.route(a, b, &mut scratch)?;
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            result = Some(r);
        }
        let (route, stats) = result.ok_or("no samples")?;
        let route = route.ok_or("no route")?;
        println!(
            "route_16km2: straight_m={:.0} route_m={:.0} waypoints={} expanded={} p50_ms={:.4} p95_ms={:.4}",
            ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt(),
            route.cost,
            route.waypoints.len(),
            stats.expanded,
            percentile(&mut samples, 0.5),
            percentile(&mut samples, 0.95),
        );
    }
    // Resident-set assembly for a 3 x 3 cell window (worker-side cost).
    let mut streamer = streamer;
    let window: BTreeSet<CellCoord> = (0..3)
        .flat_map(|x| (0..3).map(move |z| CellCoord { x, z }))
        .collect();
    let start = Instant::now();
    streamer.request(window);
    streamer.wait().ok_or("no assembly")??;
    println!(
        "stream_assembly: cells=9 ms={:.2}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    large_world()?;
    for threads in [1, 2, 4, 8] {
        crowd(CrowdSolver::Orca, threads, 10_000)?;
    }
    crowd(CrowdSolver::Detour, 1, 2_000)?;
    crowd(CrowdSolver::Orca, 1, 2_000)?;
    Ok(())
}
