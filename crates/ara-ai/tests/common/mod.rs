#![allow(dead_code, clippy::unwrap_used)]
//! Headless scenario harness: baked map, navigation + AI systems, scripted actors.
use ara_ai::*;
use ara_nav::*;
use ara_perception::{NoiseEvents, Perceiver, PerceptionPose, Stimulus};
use bevy_ecs::{prelude::*, schedule::Schedule};

pub const DT: f32 = 1.0 / 60.0;

/// Floor quad (upward winding).
pub fn floor(g: &mut Geometry, x: [f32; 2], z: [f32; 2], y: f32, area: u8) {
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

/// Solid 3 m box with vertical faces (blocks walking and navmesh sight).
pub fn solid(g: &mut Geometry, x: [f32; 2], z: [f32; 2]) {
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

/// Bakes geometry over `size` metres with 0.25 m voxels.
pub fn bake(geometry: Geometry, size: [f32; 2]) -> NavMesh {
    let settings = BakeSettings {
        origin: [0.0, -2.0, 0.0],
        max_y: 6.0,
        cell_size: 0.25,
        tile_size: 64,
        ..Default::default()
    };
    let tiles = BakeInput::tiles_in(
        &settings,
        Bounds {
            min: [0.0, -1.0, 0.0],
            max: [size[0], 1.0, size[1]],
        },
    )
    .unwrap();
    let input = BakeInput {
        settings,
        geometry,
        links: vec![],
        obstacles: vec![],
        tiles,
    };
    NavMesh::from_artifact(&Baker::new(None).bake(&input, None).unwrap()).unwrap()
}

/// World with navigation (ORCA) and AI on one fixed schedule.
pub struct Sim {
    pub world: World,
    pub schedule: Schedule,
    pub tick: u64,
    pub log: Vec<String>,
}

impl Sim {
    pub fn new(mesh: NavMesh) -> Self {
        let mut world = World::new();
        let mut schedule = Schedule::default();
        ara_ai::register_sim(&mut world, &mut schedule, AiConfig::default());
        ara_nav::register_sim(&mut world, &mut schedule, NavConfig::default()).unwrap();
        world.resource_mut::<NavWorld>().stage(mesh);
        // Publish before the first AI tick (a loaded level starts with its mesh).
        ara_nav::tick(&mut world);
        Self {
            world,
            schedule,
            tick: 0,
            log: Vec::new(),
        }
    }

    pub fn load(&mut self, key: &str, file: &str) {
        let text = std::fs::read_to_string(format!(
            "{}/behaviors/{file}.behavior",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        self.world
            .resource_mut::<AiWorld>()
            .load(key, &text)
            .unwrap();
    }

    /// An AI agent: navigation, senses and a brain with blackboard overrides.
    pub fn npc(
        &mut self,
        id: u64,
        key: &str,
        at: [f32; 3],
        senses: Perceiver,
        overrides: &[&str],
    ) -> Entity {
        self.world
            .spawn((
                NavAgent::default(),
                NavPosition(at),
                senses,
                Brain {
                    behavior: None,
                    overrides: overrides.iter().map(|s| (*s).to_owned()).collect(),
                    enabled: true,
                },
            ))
            .insert(bundle(id, key, at))
            .id()
    }

    /// A scripted stimulus (the "player").
    pub fn actor(&mut self, id: u64, kind: &str, at: [f32; 3]) -> Entity {
        self.world
            .spawn((
                Stimulus {
                    kind: kind.into(),
                    ..Default::default()
                },
                PerceptionPose {
                    id,
                    position: at,
                    forward: [0.0, 0.0, 1.0],
                },
            ))
            .id()
    }

    pub fn noise(&mut self, at: [f32; 3], radius: f32) {
        self.world
            .resource_mut::<NoiseEvents>()
            .push(ara_perception::Noise {
                source: 0,
                position: at,
                radius,
                kind: 0,
            });
    }

    /// Runs one tick and logs state changes as `tick brain from -> to (cause)`.
    pub fn step(&mut self) {
        self.schedule.run(&mut self.world);
        self.tick += 1;
        let ai = self.world.resource::<AiWorld>();
        assert!(ai.errors().is_empty(), "{:?}", ai.errors());
        for e in ai.trace() {
            if let TraceEvent::Transition {
                brain,
                from,
                to,
                cause,
            } = e
            {
                self.log.push(format!(
                    "{:05} #{brain} {} -> {to} ({cause})",
                    self.tick,
                    from.as_deref().unwrap_or("-")
                ));
            }
        }
    }

    pub fn leaf(&mut self, entity: Entity) -> String {
        view(&self.world, entity)
            .map(|v| v.path.last().cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn position(&self, entity: Entity) -> [f32; 3] {
        self.world.get::<PerceptionPose>(entity).unwrap().position
    }

    pub fn move_actor(&mut self, entity: Entity, to: [f32; 3]) {
        let mut pose = self.world.get_mut::<PerceptionPose>(entity).unwrap();
        let from = pose.position;
        pose.forward = [to[0] - from[0], 0.0, to[2] - from[2]];
        pose.position = to;
    }

    pub fn set_var(&mut self, entity: Entity, name: &str, value: Value) {
        let key = self
            .world
            .get::<BrainState>(entity)
            .unwrap()
            .program
            .clone();
        let slot = self
            .world
            .resource::<AiWorld>()
            .program(&key)
            .unwrap()
            .slot(name)
            .unwrap();
        self.world.get_mut::<BrainState>(entity).unwrap().vars[slot as usize] = value;
    }

    pub fn var(&self, entity: Entity, name: &str) -> Value {
        let state = self.world.get::<BrainState>(entity).unwrap();
        let slot = self
            .world
            .resource::<AiWorld>()
            .program(&state.program)
            .unwrap()
            .slot(name)
            .unwrap();
        state.vars[slot as usize].clone()
    }
}

/// Moves `from` toward `to` by at most `step`; returns the new point.
pub fn toward(from: [f32; 3], to: [f32; 3], step: f32) -> [f32; 3] {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len <= step {
        to
    } else {
        std::array::from_fn(|i| from[i] + d[i] / len * step)
    }
}

/// Compares a log with `tests/golden/<name>.txt`; `ARA_UPDATE_AI_GOLDENS=1` rewrites it.
pub fn golden(name: &str, log: &[String]) {
    let path = format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    let text = log.join("\n") + "\n";
    if std::env::var("ARA_UPDATE_AI_GOLDENS").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("missing golden {path}; run with ARA_UPDATE_AI_GOLDENS=1 and inspect")
    });
    assert_eq!(
        text.replace("\r\n", "\n"),
        expected.replace("\r\n", "\n"),
        "golden {name} differs"
    );
}
