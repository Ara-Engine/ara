use crate::model::{self, Grid};
use bevy_ecs::{prelude::*, world::CommandQueue};
use glam::Vec4;
use rayon::prelude::*;

#[derive(Component)]
pub struct Position(pub Vec4);
#[derive(Component)]
pub struct Velocity(pub Vec4);
#[derive(Component)]
pub struct Rotation(pub Vec4);
#[derive(Component)]
pub struct StableId(pub u32);

/// Typed structural intent recorded by Rust or C# before Bevy command playback.
#[repr(C)]
#[derive(Default, Copy, Clone)]
pub struct ChurnCommand {
    pub slot: u32,
    pub id: u32,
    pub p: [f32; 4],
    pub v: [f32; 4],
    pub q: [f32; 4],
}

pub fn record_churn(command: &mut ChurnCommand, i: usize, frame: u32) {
    let id = 100_000 + frame * 10_000 + i as u32;
    let (p, v, q) = model::initial(id);
    *command = ChurnCommand {
        slot: (frame % 10) * 10_000 + i as u32,
        id,
        p: p.to_array(),
        v: v.to_array(),
        q: q.to_array(),
    };
}

/// Bevy world, stable IDs for snapshots, and reusable per-workload scratch space.
pub struct BenchWorld {
    pub world: World,
    entities: Vec<Entity>,
    pub commands: Vec<ChurnCommand>,
    queue: CommandQueue,
    p: Vec<Vec4>,
    v: Vec<Vec4>,
    grid: Grid,
}

impl BenchWorld {
    pub fn new(n: usize) -> Self {
        let mut world = World::new();
        let entities = (0..n)
            .map(|id| {
                let (p, v, q) = model::initial(id as u32);
                world
                    .spawn((StableId(id as u32), Position(p), Velocity(v), Rotation(q)))
                    .id()
            })
            .collect();
        Self {
            world,
            entities,
            commands: vec![ChurnCommand::default(); 10_000],
            queue: CommandQueue::default(),
            p: vec![Vec4::ZERO; n],
            v: vec![Vec4::ZERO; n],
            grid: Grid::new(n),
        }
    }

    pub fn step(&mut self, workload: &str, parallel: bool, frame: u32) {
        match workload {
            "w1" => {
                let mut query = self
                    .world
                    .query::<(&mut Position, &Velocity, &mut Rotation)>();
                let update = |(mut p, v, mut q): (Mut<Position>, &Velocity, Mut<Rotation>)| {
                    model::transform(&mut p.0, v.0, &mut q.0)
                };
                if parallel {
                    query.par_iter_mut(&mut self.world).for_each(update);
                } else {
                    query.iter_mut(&mut self.world).for_each(update);
                }
            }
            "w2" => {
                for (id, p, v) in self
                    .world
                    .query::<(&StableId, &Position, &Velocity)>()
                    .iter(&self.world)
                {
                    self.p[id.0 as usize] = p.0;
                    self.v[id.0 as usize] = v.0;
                }
                self.grid.build(&self.p);
                let mut query = self
                    .world
                    .query::<(&StableId, &mut Position, &mut Velocity)>();
                let update = |(id, mut p, mut v): (&StableId, Mut<Position>, Mut<Velocity>)| {
                    (p.0, v.0) = self.grid.boid(id.0 as usize, &self.p, &self.v);
                };
                if parallel {
                    query.par_iter_mut(&mut self.world).for_each(update);
                } else {
                    query.iter_mut(&mut self.world).for_each(update);
                }
            }
            "w3" => {
                let mut query = self.world.query::<(&mut Position, &mut Velocity)>();
                let update = |(mut p, mut v): (Mut<Position>, Mut<Velocity>)| {
                    model::particle(&mut p.0, &mut v.0)
                };
                if parallel {
                    query.par_iter_mut(&mut self.world).for_each(update);
                } else {
                    query.iter_mut(&mut self.world).for_each(update);
                }
            }
            "w4" => {
                if parallel {
                    self.commands
                        .par_iter_mut()
                        .enumerate()
                        .for_each(|(i, c)| record_churn(c, i, frame));
                } else {
                    self.commands
                        .iter_mut()
                        .enumerate()
                        .for_each(|(i, c)| record_churn(c, i, frame));
                }
                self.playback();
            }
            _ => panic!("unknown workload"),
        }
    }

    /// Includes recording deferred ECS operations and applying them, in that order.
    pub fn playback(&mut self) {
        let mut commands = Commands::new(&mut self.queue, &self.world);
        for c in &self.commands {
            commands.entity(self.entities[c.slot as usize]).despawn();
            self.entities[c.slot as usize] = commands
                .spawn((
                    StableId(c.id),
                    Position(Vec4::from_array(c.p)),
                    Velocity(Vec4::from_array(c.v)),
                    Rotation(Vec4::from_array(c.q)),
                ))
                .id();
        }
        self.queue.apply(&mut self.world);
    }

    /// Sort by logical slot, independent of ECS allocation/playback order.
    pub fn records(&mut self) -> Vec<[f32; 12]> {
        let n = self.entities.len();
        assert_eq!(
            self.world.query::<&StableId>().iter(&self.world).count(),
            n,
            "live entity count is invariant"
        );
        let mut records = vec![[0.0; 12]; n];
        for (id, p, v, q) in self
            .world
            .query::<(&StableId, &Position, &Velocity, &Rotation)>()
            .iter(&self.world)
        {
            let record = &mut records[id.0 as usize % n];
            record[..4].copy_from_slice(&p.0.to_array());
            record[4..8].copy_from_slice(&v.0.to_array());
            record[8..].copy_from_slice(&q.0.to_array());
        }
        records
    }
}
