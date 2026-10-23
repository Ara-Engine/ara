//! 1,000 NPCs (guards and soldiers) with perception, brains and ORCA navigation
//! on a pillared 160 m yard: per-tick cost at 1/4/8 workers.
//! `cargo bench -p ara-ai --bench ai` (CARGO_BUILD_JOBS=2 on shared hosts).
use ara_ai::*;
use ara_nav::*;
use ara_perception::{NoiseEvents, Perceiver, PerceptionPose, Stimulus};
use bevy_ecs::{prelude::World, schedule::Schedule};
use std::time::Instant;

fn floor(g: &mut Geometry, x: [f32; 2], z: [f32; 2], y: f32, area: u8) {
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

fn solid(g: &mut Geometry, x: [f32; 2], z: [f32; 2]) {
    floor(g, x, z, 3.0, 255);
    let mut side = |a: [f32; 2], b: [f32; 2]| {
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
    side([x[0], z[0]], [x[1], z[0]]);
    side([x[1], z[0]], [x[1], z[1]]);
    side([x[1], z[1]], [x[0], z[1]]);
    side([x[0], z[1]], [x[0], z[0]]);
}

fn behavior(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/behaviors/{name}.behavior",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("example behaviour")
}

fn run(threads: usize, mesh: &NavMesh) -> Result<(), Box<dyn std::error::Error>> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;
    let mut world = World::new();
    let mut schedule = Schedule::default();
    ara_ai::register_sim(&mut world, &mut schedule, AiConfig::default());
    ara_nav::register_sim(&mut world, &mut schedule, NavConfig::default())?;
    world.resource_mut::<NavWorld>().stage(mesh.clone());
    ara_nav::tick(&mut world);
    {
        let mut ai = world.resource_mut::<AiWorld>();
        ai.load("guard", &behavior("guard"))
            .map_err(|d| format!("{d:?}"))?;
        ai.load("soldier", &behavior("soldier"))
            .map_err(|d| format!("{d:?}"))?;
    }
    for id in 1..=1000u64 {
        let (x, z) = (4.0 + (id % 40) as f32 * 3.9, 4.0 + (id / 40) as f32 * 6.0);
        let key = if id % 4 == 0 { "soldier" } else { "guard" };
        let route = format!(
            "route = [vec3({x}, 0, {z}), vec3({}, 0, {z})]",
            (x + 6.0).min(156.0)
        );
        world
            .spawn((
                NavAgent::default(),
                NavPosition([x, 0.1, z]),
                Perceiver::default(),
                Brain {
                    behavior: None,
                    overrides: vec![route],
                    enabled: true,
                },
            ))
            .insert(bundle(id, key, [x, 0.1, z]));
    }
    for k in 0..8u64 {
        world.spawn((
            Stimulus {
                kind: "player".into(),
                ..Default::default()
            },
            PerceptionPose {
                id: 10_000 + k,
                position: [10.0 + k as f32 * 18.0, 0.1, 80.0],
                forward: [0.0, 0.0, 1.0],
            },
        ));
    }
    let mut samples = Vec::new();
    for tick in 0..600u32 {
        if tick % 60 == 0 {
            world
                .resource_mut::<NoiseEvents>()
                .push(ara_perception::Noise {
                    source: 0,
                    position: [(tick % 150) as f32 + 5.0, 0.0, 70.0],
                    radius: 30.0,
                    kind: 0,
                });
        }
        let start = Instant::now();
        pool.install(|| schedule.run(&mut world));
        if tick >= 60 {
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    samples.sort_by(f64::total_cmp);
    let transitions = world.resource::<AiWorld>().trace().len();
    println!(
        "ai_tick: npcs=1000 workers={threads} p50_ms={:.3} p95_ms={:.3} last_tick_events={transitions}",
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100]
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 160.0], [0.0, 160.0], 0.0, 0);
    for k in 0..36 {
        let (x, z) = (12.0 + (k % 6) as f32 * 25.0, 12.0 + (k / 6) as f32 * 25.0);
        solid(&mut g, [x, x + 3.0], [z, z + 3.0]);
    }
    let settings = BakeSettings {
        origin: [0.0, -2.0, 0.0],
        max_y: 6.0,
        cell_size: 0.25,
        tile_size: 128,
        ..Default::default()
    };
    let input = BakeInput {
        tiles: BakeInput::tiles_in(
            &settings,
            Bounds {
                min: [0.0, -1.0, 0.0],
                max: [160.0, 1.0, 160.0],
            },
        )?,
        settings,
        geometry: g,
        links: vec![],
        obstacles: vec![],
    };
    let mesh = NavMesh::from_artifact(&Baker::new(None).bake(&input, None)?)?;
    for threads in [1, 4, 8] {
        run(threads, &mesh)?;
    }
    Ok(())
}
