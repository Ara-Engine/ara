//! Declared scheduler dependencies with independent B/C component columns.
use super::model::{Frame, Id, StageA, StageB, StageC, Value, mix};
use bevy_ecs::prelude::*;

fn a(frame: Res<Frame>, mut query: Query<(&Value, &mut StageA)>) {
    let tick = frame.tick;
    let run = |(value, mut out): (&Value, Mut<StageA>)| {
        out.0 = mix(value.value.wrapping_add(tick));
    };
    if frame.parallel {
        query.par_iter_mut().for_each(run);
    } else {
        query.iter_mut().for_each(run);
    }
}
fn b(frame: Res<Frame>, mut query: Query<(&Id, &StageA, &mut StageB)>) {
    let run = |(id, a, mut out): (&Id, &StageA, Mut<StageB>)| {
        out.0 = a.0.wrapping_mul(3).wrapping_add(id.0);
    };
    if frame.parallel {
        query.par_iter_mut().for_each(run);
    } else {
        query.iter_mut().for_each(run);
    }
}
fn c(frame: Res<Frame>, mut query: Query<(&Id, &StageA, &mut StageC)>) {
    let tick = frame.tick;
    let run = |(id, a, mut out): (&Id, &StageA, Mut<StageC>)| {
        out.0 = a.0 ^ id.0.wrapping_mul(5).wrapping_add(tick);
    };
    if frame.parallel {
        query.par_iter_mut().for_each(run);
    } else {
        query.iter_mut().for_each(run);
    }
}
fn commit(frame: Res<Frame>, mut query: Query<(&StageB, &StageC, &mut Value)>) {
    let run = |(b, c, mut value): (&StageB, &StageC, Mut<Value>)| {
        value.value = b.0.wrapping_add(c.0);
        value.updates += 1;
    };
    if frame.parallel {
        query.par_iter_mut().for_each(run);
    } else {
        query.iter_mut().for_each(run);
    }
}

pub fn schedule(parallel: bool, fork_join: bool) -> Schedule {
    let mut schedule = super::model::configured_schedule(parallel);
    if fork_join {
        schedule.add_systems((
            a.before(b).before(c),
            b.before(commit),
            c.before(commit),
            commit,
        ));
    } else {
        schedule.add_systems((a, b, c, commit).chain());
    }
    schedule
}
