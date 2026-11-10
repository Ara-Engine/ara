//! Stable-order typed command playback, followed by a retained TagD query.
use super::model::{Frame, Id, TagD, Value, update};
use bevy_ecs::{prelude::*, world::CommandQueue};
use bevy_tasks::ComputeTaskPool;

pub struct Churn {
    selected: Vec<Entity>,
    queues: Vec<CommandQueue>,
    batch: usize,
    query: QueryState<(&'static Id, &'static mut Value), With<TagD>>,
}

impl Churn {
    pub fn new(world: &mut World, entities: &[Entity], percent: u32, batch: usize) -> Self {
        let selected: Vec<_> = entities
            .iter()
            .enumerate()
            .filter(|(id, _)| *id as u32 % 100 < percent)
            .map(|(_, entity)| *entity)
            .collect();
        let queues = (0..selected.len().div_ceil(batch))
            .map(|_| CommandQueue::default())
            .collect();
        Self {
            selected,
            queues,
            batch,
            query: world.query_filtered(),
        }
    }

    pub fn step(&mut self, world: &mut World, frame: Frame) {
        let add = frame.tick.is_multiple_of(2);
        let world_ref = &*world;
        if frame.parallel {
            ComputeTaskPool::get().scope(|scope| {
                for (entities, queue) in self.selected.chunks(self.batch).zip(&mut self.queues) {
                    scope.spawn(async move {
                        record(queue, world_ref, entities, add);
                    });
                }
            });
        } else {
            for (entities, queue) in self.selected.chunks(self.batch).zip(&mut self.queues) {
                record(queue, world_ref, entities, add);
            }
        }
        // Each queue contains ascending IDs; apply ascending batch order after the join.
        for queue in &mut self.queues {
            queue.apply(world);
        }
        let run = |(id, mut value): (&Id, Mut<Value>)| {
            value.value = update(value.value, id.0, frame.tick);
            value.updates += 1;
        };
        if frame.parallel {
            self.query.par_iter_mut(world).for_each(run);
        } else {
            self.query.iter_mut(world).for_each(run);
        }
    }
}

fn record(queue: &mut CommandQueue, world: &World, entities: &[Entity], add: bool) {
    let mut commands = Commands::new(queue, world);
    for &entity in entities {
        if add {
            commands.entity(entity).insert(TagD);
        } else {
            commands.entity(entity).remove::<TagD>();
        }
    }
}
