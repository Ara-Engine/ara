//! Safe contiguous chunk dispatch, slice kernels and batched change detection.
use super::baseline::{BenchWorld, Position, Rotation, StableId, Velocity};
use crate::model;
use bevy_ecs::{batching::BatchingStrategy, change_detection::ContiguousMut};

impl BenchWorld {
    /// Times W2's three phases separately to locate the remaining parallel gap.
    /// This diagnostic path is separate from the matched workload timing path.
    pub fn profile_boids(&mut self, parallel: bool, batch: usize) -> [f64; 3] {
        let start = std::time::Instant::now();
        for (ids, p, v) in self
            .world
            .query::<(&StableId, &Position, &Velocity)>()
            .contiguous_iter(&self.world)
            .expect("table-only query")
        {
            for ((id, p), v) in ids.iter().zip(p).zip(v) {
                self.p[id.0 as usize] = p.0;
                self.v[id.0 as usize] = v.0;
            }
        }
        let snapshot_done = std::time::Instant::now();
        self.sorted.build(&self.p, &self.v);
        let grid_done = std::time::Instant::now();
        let mut query = self
            .world
            .query::<(&StableId, &mut Position, &mut Velocity)>();
        let update = |(ids, p, v): (
            &[StableId],
            ContiguousMut<Position>,
            ContiguousMut<Velocity>,
        )| {
            for ((id, p), v) in ids
                .iter()
                .zip(ContiguousMut::split(p).0)
                .zip(ContiguousMut::split(v).0)
            {
                (p.0, v.0) = self.sorted.boid(id.0 as usize, &self.p, &self.v);
            }
        };
        if parallel {
            query
                .contiguous_par_iter_mut(&mut self.world)
                .expect("table-only query")
                .batching_strategy(BatchingStrategy::fixed(batch))
                .for_each(update);
        } else {
            query
                .contiguous_iter_mut(&mut self.world)
                .expect("table-only query")
                .for_each(update);
        }
        let done = std::time::Instant::now();
        [
            snapshot_done.duration_since(start).as_secs_f64() * 1000.,
            grid_done.duration_since(snapshot_done).as_secs_f64() * 1000.,
            done.duration_since(grid_done).as_secs_f64() * 1000.,
        ]
    }

    /// Measures the contiguous-query and spatial-layout changes without SIMD batching.
    pub fn step_tuned(&mut self, workload: &str, parallel: bool, frame: u32, batch: usize) {
        self.step_kernel(workload, parallel, frame, batch, false);
    }

    /// Measures the final chunk/spatial/SIMD implementation; W4 retains the baseline.
    pub fn step_simd(&mut self, workload: &str, parallel: bool, frame: u32, batch: usize) {
        self.step_kernel(workload, parallel, frame, batch, true);
    }

    fn step_kernel(
        &mut self,
        workload: &str,
        parallel: bool,
        frame: u32,
        batch: usize,
        simd: bool,
    ) {
        match workload {
            "w1" => {
                let mut query = self
                    .world
                    .query::<(&mut Position, &Velocity, &mut Rotation)>();
                let update = |(p, v, q): (
                    ContiguousMut<Position>,
                    &[Velocity],
                    ContiguousMut<Rotation>,
                )| {
                    if simd {
                        super::kernels::transforms(
                            ContiguousMut::split(p).0,
                            v,
                            ContiguousMut::split(q).0,
                        );
                        return;
                    }
                    for ((p, v), q) in ContiguousMut::split(p)
                        .0
                        .iter_mut()
                        .zip(v)
                        .zip(ContiguousMut::split(q).0)
                    {
                        model::transform(&mut p.0, v.0, &mut q.0);
                    }
                };
                if parallel {
                    query
                        .contiguous_par_iter_mut(&mut self.world)
                        .expect("table-only query")
                        .batching_strategy(BatchingStrategy::fixed(batch))
                        .for_each(update);
                } else {
                    query
                        .contiguous_iter_mut(&mut self.world)
                        .expect("table-only query")
                        .for_each(update);
                }
            }
            "w2" => {
                for (ids, p, v) in self
                    .world
                    .query::<(&StableId, &Position, &Velocity)>()
                    .contiguous_iter(&self.world)
                    .expect("table-only query")
                {
                    for ((id, p), v) in ids.iter().zip(p).zip(v) {
                        self.p[id.0 as usize] = p.0;
                        self.v[id.0 as usize] = v.0;
                    }
                }
                self.sorted.build(&self.p, &self.v);
                let mut query = self
                    .world
                    .query::<(&StableId, &mut Position, &mut Velocity)>();
                let update = |(ids, p, v): (
                    &[StableId],
                    ContiguousMut<Position>,
                    ContiguousMut<Velocity>,
                )| {
                    for ((id, p), v) in ids
                        .iter()
                        .zip(ContiguousMut::split(p).0)
                        .zip(ContiguousMut::split(v).0)
                    {
                        (p.0, v.0) = self.sorted.boid(id.0 as usize, &self.p, &self.v);
                    }
                };
                if parallel {
                    query
                        .contiguous_par_iter_mut(&mut self.world)
                        .expect("table-only query")
                        .batching_strategy(BatchingStrategy::fixed(batch))
                        .for_each(update);
                } else {
                    query
                        .contiguous_iter_mut(&mut self.world)
                        .expect("table-only query")
                        .for_each(update);
                }
            }
            "w3" => {
                let mut query = self.world.query::<(&mut Position, &mut Velocity)>();
                let update = |(p, v): (ContiguousMut<Position>, ContiguousMut<Velocity>)| {
                    if simd {
                        super::kernels::particles(
                            ContiguousMut::split(p).0,
                            ContiguousMut::split(v).0,
                        );
                        return;
                    }
                    for (p, v) in ContiguousMut::split(p)
                        .0
                        .iter_mut()
                        .zip(ContiguousMut::split(v).0)
                    {
                        model::particle(&mut p.0, &mut v.0);
                    }
                };
                if parallel {
                    query
                        .contiguous_par_iter_mut(&mut self.world)
                        .expect("table-only query")
                        .batching_strategy(BatchingStrategy::fixed(batch))
                        .for_each(update);
                } else {
                    query
                        .contiguous_iter_mut(&mut self.world)
                        .expect("table-only query")
                        .for_each(update);
                }
            }
            "w4" => self.step(workload, parallel, frame),
            _ => panic!("unknown benchmark workload"),
        }
    }
}
