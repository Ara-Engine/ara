//! 10,000-agent ORCA step timings at 1/2/4/8 workers, plus flow-field builds.
//! `cargo bench -p ara-crowd --bench crowd` (CARGO_BUILD_JOBS=2 on shared hosts).
//! `ARA_CROWD_NEIGHBORS=n` overrides `Params::max_neighbors` for comparisons.
use ara_crowd::*;
use std::time::Instant;

/// Four touching 50 x 50 blocks (2,500 agents each) swap to the opposite corner,
/// interpenetrating in a dense four-way crossing inside a walled 200 m arena.
fn scenario() -> (Vec<Agent>, Vec<[f32; 3]>) {
    let mut agents = Vec::with_capacity(10_000);
    let mut goals = Vec::with_capacity(10_000);
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        for i in 0..2500 {
            let (gx, gz) = ((i % 50) as f32 * 1.2, (i / 50) as f32 * 1.2);
            let x = sx * (1.0 + gx);
            let z = sz * (1.0 + gz);
            agents.push(Agent {
                id: agents.len() as u64,
                position: [x, 0.0, z],
                walls: 0,
                radius: 0.3,
                max_speed: 1.6,
                ..Agent::default()
            });
            goals.push([-x, 0.0, -z]);
        }
    }
    (agents, goals)
}

fn arena() -> Walls {
    let h = 100.0;
    let mut walls = Vec::new();
    // Perimeter split into 4 m segments, like navmesh boundary edges.
    for k in 0..50 {
        let a = -h + k as f32 * 4.0;
        let b = a + 4.0;
        walls.push(Wall {
            a: [a, 0.0, -h],
            b: [b, 0.0, -h],
        });
        walls.push(Wall {
            a: [a, 0.0, h],
            b: [b, 0.0, h],
        });
        walls.push(Wall {
            a: [-h, 0.0, a],
            b: [-h, 0.0, b],
        });
        walls.push(Wall {
            a: [h, 0.0, a],
            b: [h, 0.0, b],
        });
    }
    Walls::new(&walls, 4.0).expect("finite walls")
}

fn steer(agents: &mut [Agent], goals: &[[f32; 3]]) {
    for (a, g) in agents.iter_mut().zip(goals) {
        let (dx, dz) = (g[0] - a.position[0], g[2] - a.position[2]);
        let len = (dx * dx + dz * dz).sqrt();
        a.preferred = if len > 0.05 {
            [dx / len * a.max_speed, 0.0, dz / len * a.max_speed]
        } else {
            [0.0; 3]
        };
    }
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() - 1) as f64 * p) as usize]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let walls = arena();
    let mut reference: Option<Vec<u32>> = None;
    for threads in [1usize, 2, 4, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()?;
        let (mut agents, goals) = scenario();
        let neighbors = std::env::var("ARA_CROWD_NEIGHBORS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(Params::default().max_neighbors);
        let mut crowd = Crowd::new(Params {
            max_neighbors: neighbors,
            ..Params::default()
        })?;
        let mut samples = Vec::new();
        let mut fallbacks = 0;
        // 600 ticks = 10 s: the blocks meet and fully interpenetrate at the centre.
        for tick in 0..600 {
            steer(&mut agents, &goals);
            let start = Instant::now();
            let stats = pool.install(|| crowd.step(&mut agents, &[&walls], 1.0 / 60.0))?;
            if tick >= 60 {
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            fallbacks += stats.fallbacks;
        }
        let hash: Vec<u32> = agents
            .iter()
            .flat_map(|a| a.position.map(f32::to_bits))
            .collect();
        let identical = reference.get_or_insert_with(|| hash.clone()) == &hash;
        let p50 = percentile(&mut samples, 0.5);
        let p95 = percentile(&mut samples, 0.95);
        let max = percentile(&mut samples, 1.0);
        println!(
            "orca_10k: workers={threads} ticks={} p50_ms={p50:.3} p95_ms={p95:.3} max_ms={max:.3} fallbacks={fallbacks} identical_to_1_worker={identical}",
            samples.len()
        );
    }

    // Flow field over a 512 m x 512 m open area at 1 m cells (262k cells).
    let mut grid = WalkGrid::new([-256.0, -256.0], [256.0, 256.0], 1.0)?;
    let mut triangles = Vec::new();
    for k in 0..16 {
        let z0 = -256.0 + k as f32 * 32.0;
        let z1 = z0 + 32.0;
        triangles.push([[-256.0, 0.0, z0], [256.0, 0.0, z1], [256.0, 0.0, z0]]);
        triangles.push([[-256.0, 0.0, z0], [-256.0, 0.0, z1], [256.0, 0.0, z1]]);
    }
    grid.rasterize(&triangles, 1);
    let start = Instant::now();
    let field = FlowField::build(&grid, [200.0, 0.0, 200.0])?;
    let build = start.elapsed().as_secs_f64() * 1000.0;
    let (mut agents, _) = scenario();
    let start = Instant::now();
    for a in &mut agents {
        let [dx, dz] = field.direction(a.position).unwrap_or([0.0, 0.0]);
        a.preferred = [dx * a.max_speed, 0.0, dz * a.max_speed];
    }
    let sample = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "flow_field: cells={} build_ms={build:.3} steer_10k_ms={sample:.4}",
        grid.walkable_cells()
    );
    Ok(())
}
