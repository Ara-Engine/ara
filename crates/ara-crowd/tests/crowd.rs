#![allow(clippy::unwrap_used)]
use ara_crowd::*;

/// Steers each agent toward its goal at full speed (simple path follower).
fn steer(agents: &mut [Agent], goals: &[[f32; 3]]) {
    for (a, g) in agents.iter_mut().zip(goals) {
        let (dx, dz) = (g[0] - a.position[0], g[2] - a.position[2]);
        let len = (dx * dx + dz * dz).sqrt();
        a.preferred = if len > 0.05 {
            let speed = a.max_speed.min(len * 60.0);
            [dx / len * speed, 0.0, dz / len * speed]
        } else {
            [0.0; 3]
        };
    }
}

fn counterflow(per_side: usize) -> (Vec<Agent>, Vec<[f32; 3]>) {
    let mut agents = Vec::new();
    let mut goals = Vec::new();
    for side in 0..2 {
        for i in 0..per_side {
            let row = (i / 10) as f32;
            let col = (i % 10) as f32;
            let x = if side == 0 { -10.0 - row } else { 10.0 + row };
            let z = col * 1.0 - 4.5 + if side == 0 { 0.0 } else { 0.25 };
            agents.push(Agent {
                id: agents.len() as u64,
                position: [x, 0.0, z],
                ..Agent::default()
            });
            goals.push([-x, 0.0, z]);
        }
    }
    (agents, goals)
}

fn bits(agents: &[Agent]) -> Vec<u32> {
    agents
        .iter()
        .flat_map(|a| a.position.iter().chain(&a.velocity).map(|v| v.to_bits()))
        .collect()
}

#[test]
fn counterflow_reaches_goals_without_overlap() {
    let (mut agents, goals) = counterflow(60);
    let mut crowd = Crowd::new(Params::default()).unwrap();
    let mut worst = f32::INFINITY;
    for _ in 0..1800 {
        steer(&mut agents, &goals);
        crowd.step(&mut agents, &[], 1.0 / 60.0).unwrap();
        for i in 0..agents.len() {
            for j in i + 1..agents.len() {
                let (a, b) = (agents[i].position, agents[j].position);
                let d = ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
                worst = worst.min(d);
            }
        }
    }
    // Two radii are 0.6 m; reciprocal avoidance keeps close to that bound.
    assert!(worst > 0.5, "closest approach {worst}");
    let arrived = agents
        .iter()
        .zip(&goals)
        .filter(|(a, g)| (a.position[0] - g[0]).abs() < 0.5 && (a.position[2] - g[2]).abs() < 0.5)
        .count();
    assert!(
        arrived >= agents.len() * 95 / 100,
        "{arrived}/{} arrived",
        agents.len()
    );
}

#[test]
fn results_are_bit_identical_on_1_4_and_8_workers() {
    let run = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let (mut agents, goals) = counterflow(400);
            let mut crowd = Crowd::new(Params::default()).unwrap();
            for _ in 0..120 {
                steer(&mut agents, &goals);
                crowd.step(&mut agents, &[], 1.0 / 60.0).unwrap();
            }
            bits(&agents)
        })
    };
    let one = run(1);
    assert_eq!(one, run(4));
    assert_eq!(one, run(8));
}

#[test]
fn results_do_not_depend_on_input_order() {
    let (mut forward, goals) = counterflow(200);
    let mut reversed: Vec<_> = forward.iter().copied().rev().collect();
    let reversed_goals: Vec<_> = goals.iter().copied().rev().collect();
    let mut a = Crowd::new(Params::default()).unwrap();
    let mut b = Crowd::new(Params::default()).unwrap();
    for _ in 0..90 {
        steer(&mut forward, &goals);
        steer(&mut reversed, &reversed_goals);
        a.step(&mut forward, &[], 1.0 / 60.0).unwrap();
        b.step(&mut reversed, &[], 1.0 / 60.0).unwrap();
    }
    reversed.reverse();
    assert_eq!(bits(&forward), bits(&reversed));
}

#[test]
fn agents_never_cross_corridor_walls() {
    // A 3 m wide corridor along X with a pillar in the middle.
    let walls = vec![
        Wall {
            a: [-20.0, 0.0, -1.5],
            b: [20.0, 0.0, -1.5],
        },
        Wall {
            a: [-20.0, 0.0, 1.5],
            b: [20.0, 0.0, 1.5],
        },
        Wall {
            a: [-0.5, 0.0, -0.5],
            b: [0.5, 0.0, -0.5],
        },
        Wall {
            a: [0.5, 0.0, -0.5],
            b: [0.5, 0.0, 0.5],
        },
        Wall {
            a: [0.5, 0.0, 0.5],
            b: [-0.5, 0.0, 0.5],
        },
        Wall {
            a: [-0.5, 0.0, 0.5],
            b: [-0.5, 0.0, -0.5],
        },
    ];
    let walls = Walls::new(&walls, 4.0).unwrap();
    let mut agents = Vec::new();
    let mut goals = Vec::new();
    for i in 0..30 {
        let z = -1.0 + (i % 5) as f32 * 0.5;
        let x = -15.0 - (i / 5) as f32 * 0.8;
        agents.push(Agent {
            id: i,
            position: [x, 0.0, z],
            radius: 0.2,
            walls: 0,
            ..Agent::default()
        });
        goals.push([15.0, 0.0, z]);
    }
    let mut crowd = Crowd::new(Params::default()).unwrap();
    for _ in 0..1200 {
        steer(&mut agents, &goals);
        crowd.step(&mut agents, &[&walls], 1.0 / 60.0).unwrap();
        for a in &agents {
            assert!(a.position[2].abs() < 1.5, "left corridor: {:?}", a.position);
            let inside_pillar = a.position[0].abs() < 0.5 && a.position[2].abs() < 0.5;
            assert!(!inside_pillar, "entered pillar: {:?}", a.position);
        }
    }
    let through = agents.iter().filter(|a| a.position[0] > 10.0).count();
    assert!(through >= 28, "{through}/30 passed the pillar");
}

#[test]
fn vertically_separated_agents_and_layers_do_not_interact() {
    let mut agents = vec![
        Agent {
            position: [0.0, 0.0, 0.0],
            preferred: [1.0, 0.0, 0.0],
            ..Agent::default()
        },
        Agent {
            position: [0.4, 4.0, 0.0],
            preferred: [-1.0, 0.0, 0.0],
            ..Agent::default()
        },
        Agent {
            position: [5.0, 0.0, 0.0],
            preferred: [1.0, 0.0, 0.0],
            layers: 2,
            ..Agent::default()
        },
        Agent {
            position: [5.4, 0.0, 0.0],
            preferred: [-1.0, 0.0, 0.0],
            layers: 4,
            ..Agent::default()
        },
    ];
    let mut crowd = Crowd::new(Params::default()).unwrap();
    for a in &mut agents {
        a.velocity = a.preferred;
    }
    crowd.step(&mut agents, &[], 1.0 / 60.0).unwrap();
    for a in &agents {
        assert!(
            a.velocity[2].abs() < 1e-6,
            "unexpected avoidance {:?}",
            a.velocity
        );
    }
}

#[test]
fn flow_field_routes_around_a_wall_and_rejects_bad_goals() {
    // 40 x 40 floor with a wall from z = 0..30 at x = 20 (gap at the top).
    let mut grid = WalkGrid::new([0.0, 0.0], [40.0, 40.0], 1.0).unwrap();
    let mut triangles = Vec::new();
    let mut quad = |x0: f32, x1: f32, z0: f32, z1: f32| {
        triangles.push([[x0, 0.0, z0], [x1, 0.0, z1], [x1, 0.0, z0]]);
        triangles.push([[x0, 0.0, z0], [x0, 0.0, z1], [x1, 0.0, z1]]);
    };
    quad(0.0, 19.0, 0.0, 40.0);
    quad(21.0, 40.0, 0.0, 40.0);
    quad(19.0, 21.0, 30.0, 40.0);
    grid.rasterize(&triangles, 1);
    let field = FlowField::build(&grid, [35.0, 0.0, 5.0]).unwrap();
    let start = [5.0, 0.0, 5.0];
    let d = field.distance(start).unwrap();
    assert!(d > 60.0, "must route through the gap: {d}");
    // Following the field reaches the goal.
    let mut p = start;
    for _ in 0..2000 {
        let Some([dx, dz]) = field.direction(p) else {
            break;
        };
        p[0] += dx * 0.1;
        p[2] += dz * 0.1;
    }
    assert!(
        (p[0] - 35.0).abs() < 1.0 && (p[2] - 5.0).abs() < 1.0,
        "{p:?}"
    );
    assert!(FlowField::build(&grid, [20.0, 0.0, 5.0]).is_err());
    assert!(WalkGrid::new([0.0, 0.0], [1.0e5, 1.0e5], 1.0).is_err());
}

#[test]
fn invalid_input_is_rejected_with_the_agent_index() {
    let mut crowd = Crowd::new(Params::default()).unwrap();
    let mut agents = vec![
        Agent::default(),
        Agent {
            radius: f32::NAN,
            ..Agent::default()
        },
    ];
    let error = crowd.step(&mut agents, &[], 1.0 / 60.0).unwrap_err();
    assert!(error.0.contains("agent 1"));
    assert!(
        Crowd::new(Params {
            max_neighbors: 0,
            ..Params::default()
        })
        .is_err()
    );
}

#[test]
fn obstacle_discs_stay_put_and_are_avoided() {
    let mut agents = vec![
        Agent {
            id: 1,
            position: [-5.0, 0.0, 0.05],
            ..Agent::default()
        },
        Agent {
            id: 2,
            position: [0.0, 0.0, 0.0],
            radius: 1.0,
            obstacle: true,
            ..Agent::default()
        },
    ];
    let goals = [[5.0, 0.0, 0.05], [0.0, 0.0, 0.0]];
    let mut crowd = Crowd::new(Params::default()).unwrap();
    for _ in 0..400 {
        steer(&mut agents, &goals);
        agents[1].preferred = [0.0; 3];
        crowd.step(&mut agents, &[], 1.0 / 60.0).unwrap();
        let p = agents[0].position;
        assert!(
            (p[0] * p[0] + p[2] * p[2]).sqrt() > 1.25,
            "entered obstacle: {p:?}"
        );
    }
    assert_eq!(agents[1].position, [0.0, 0.0, 0.0]);
    assert!(agents[0].position[0] > 4.0, "{:?}", agents[0].position);
}

#[test]
fn agents_sidestep_a_disc_exactly_on_their_line() {
    let mut agents = vec![
        Agent {
            id: 1,
            position: [-8.0, 0.0, 0.0],
            walls: NO_WALLS,
            ..Agent::default()
        },
        Agent {
            id: 2,
            position: [0.0, 0.0, 0.0],
            radius: 2.0,
            obstacle: true,
            ..Agent::default()
        },
    ];
    let goals = [[8.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
    let mut crowd = Crowd::new(Params::default()).unwrap();
    for _ in 0..600 {
        steer(&mut agents, &goals);
        agents[1].preferred = [0.0; 3];
        crowd.step(&mut agents, &[], 1.0 / 60.0).unwrap();
    }
    assert!(
        agents[0].position[0] > 7.5,
        "stuck at {:?}",
        agents[0].position
    );
}
