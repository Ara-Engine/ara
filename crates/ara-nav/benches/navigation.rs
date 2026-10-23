//! Standalone measurements with bounded work; run with CARGO_BUILD_JOBS=2.
use ara_nav::*;
use bevy_ecs::{prelude::World, schedule::Schedule};
use std::{hint::black_box, time::Instant};

fn input() -> Result<BakeInput, Box<dyn std::error::Error>> {
    let settings = BakeSettings {
        tile_size: 32,
        ..Default::default()
    };
    let mut geometry = Geometry::default();
    // Dense collision input with 32768 triangles, not just two huge floor faces.
    for z in 0..128 {
        for x in 0..128 {
            let i = geometry.vertices.len() as u32;
            let x = x as f32 * 0.5;
            let z = z as f32 * 0.5;
            geometry.vertices.extend([
                [x, 0.0, z],
                [x + 0.5, 0.0, z],
                [x + 0.5, 0.0, z + 0.5],
                [x, 0.0, z + 0.5],
            ]);
            geometry
                .triangles
                .extend([[i, i + 2, i + 1], [i, i + 3, i + 2]]);
            geometry.areas.extend([0; 2]);
        }
    }
    let tiles = BakeInput::tiles_in(
        &settings,
        Bounds {
            min: [0.0, -1.0, 0.0],
            max: [64.0, 1.0, 64.0],
        },
    )?;
    Ok(BakeInput {
        settings,
        geometry,
        tiles,
        links: vec![],
        obstacles: vec![],
    })
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = input()?;
    let baker = Baker::new(None);
    let start = Instant::now();
    let artifact = baker.bake(&input, None)?;
    println!(
        "bake: triangles={} tiles={} ms={:.3}",
        input.geometry.triangles.len(),
        artifact.tiles.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    let start = Instant::now();
    let unchanged = baker.bake(&input, Some(&artifact))?;
    println!(
        "unchanged: reused={} ms={:.3}",
        unchanged.stats.reused,
        start.elapsed().as_secs_f64() * 1000.0
    );
    input.obstacles.push(Obstacle {
        id: 1,
        bounds: Bounds {
            min: [30.0, -1.0, 30.0],
            max: [32.0, 2.0, 32.0],
        },
        area: Obstacle::CARVE,
    });
    let start = Instant::now();
    let changed = baker.bake(&input, Some(&artifact))?;
    println!(
        "incremental: rebuilt={} reused={} ms={:.3}",
        changed.stats.rebuilt,
        changed.stats.reused,
        start.elapsed().as_secs_f64() * 1000.0
    );
    let mesh = NavMesh::from_artifact(&artifact)?;
    let mut query = mesh.query()?;
    let requests: Vec<_> = (0..10000)
        .map(|i| PathRequest {
            start: [2.0, 0.1, 2.0],
            end: [5.0 + (i % 55) as f32, 0.1, 55.0],
            filter: Default::default(),
        })
        .collect();
    let start = Instant::now();
    for r in &requests {
        black_box(query.path(r.start, r.end, &r.filter)?);
    }
    println!(
        "paths_sequential: count={} per_second={:.0}",
        requests.len(),
        requests.len() as f64 / start.elapsed().as_secs_f64()
    );
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build()?;
    let start = Instant::now();
    let results = pool.install(|| mesh.paths(&requests));
    assert!(results.iter().all(|p| p.as_ref().is_ok_and(|p| p.complete)));
    println!(
        "paths_2_threads: count={} per_second={:.0}",
        requests.len(),
        requests.len() as f64 / start.elapsed().as_secs_f64()
    );
    drop(query);
    let mut world = World::new();
    let mut schedule = Schedule::default();
    register_sim(&mut world, &mut schedule, Default::default())?;
    world.resource_mut::<NavWorld>().stage(mesh);
    for id in 0..500 {
        let x = 3.0 + (id % 25) as f32 * 1.2;
        let z = 3.0 + (id / 25) as f32 * 1.2;
        let mut state = NavAgentState::new(id);
        state.target = Some([x + 20.0, 0.1, z + 6.0]);
        state.status = NavStatus::Moving;
        world.spawn((NavAgent::default(), NavPosition([x, 0.1, z]), state));
    }
    schedule.run(&mut world); // Include native path preparation outside steady-state samples.
    let mut samples = Vec::with_capacity(120);
    for _ in 0..120 {
        let start = Instant::now();
        schedule.run(&mut world);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    assert!(
        world.resource::<NavWorld>().errors().is_empty(),
        "{:?}",
        world.resource::<NavWorld>().errors()
    );
    println!(
        "crowd_500: ticks={} p50_ms={:.3} p95_ms={:.3}",
        samples.len(),
        samples[60],
        samples[114]
    );
    Ok(())
}
