//! Real per-entity change filtering, with an explicit epoch for matched semantics.
use super::model::{Epoch, Frame, Id, Value, selected, update};
use bevy_ecs::prelude::*;

fn mark(frame: Res<Frame>, mut query: Query<(&Id, &mut Epoch)>) {
    let context = *frame;
    let write = |(id, mut epoch): (&Id, Mut<Epoch>)| {
        if selected(id.0, context) {
            epoch.0 = context.tick + 1;
        }
    };
    if context.parallel {
        query.par_iter_mut().for_each(write);
    } else {
        query.iter_mut().for_each(write);
    }
}

fn consume(frame: Res<Frame>, mut query: Query<(&Id, &Epoch, &mut Value), Changed<Epoch>>) {
    let context = *frame;
    let apply = |(id, epoch, mut value): (&Id, &Epoch, Mut<Value>)| {
        if epoch.0 == context.tick + 1 {
            value.value = update(value.value, id.0, context.tick);
            value.updates += 1;
        }
    };
    if context.parallel {
        query.par_iter_mut().for_each(apply);
    } else {
        query.iter_mut().for_each(apply);
    }
}

#[derive(Resource, Default)]
pub struct Counts(pub Vec<(u32, usize, usize)>);

fn observe(frame: Res<Frame>, query: Query<&Epoch, Changed<Epoch>>, mut counts: ResMut<Counts>) {
    counts.0.push((
        frame.tick,
        query.iter().count(),
        query
            .iter()
            .filter(|epoch| epoch.0 == frame.tick + 1)
            .count(),
    ));
}

pub fn schedule(parallel: bool) -> Schedule {
    let mut schedule = super::model::configured_schedule(parallel);
    schedule.add_systems((mark, consume).chain());
    schedule
}

pub fn diagnostic_schedule() -> Schedule {
    let mut schedule = super::model::configured_schedule(false);
    schedule.add_systems((mark, observe, consume).chain());
    schedule
}
