//! Separate diagnostic runner observations; never part of the throughput sample loop.
use super::model::*;
use bevy_ecs::prelude::*;
use std::{cell::Cell, collections::BTreeSet, sync::Mutex};

thread_local! { static SEEN: Cell<bool> = const { Cell::new(false) }; }

#[derive(Resource, Default)]
struct Threads(Mutex<BTreeSet<String>>);

fn record(threads: &Threads) {
    SEEN.with(|seen| {
        if !seen.replace(true) {
            threads
                .0
                .lock()
                .expect("diagnostic observer has no panicking writers")
                .insert(format!("{:?}", std::thread::current().id()));
        }
    });
}

fn a(threads: Res<Threads>, mut query: Query<(&Value, &mut StageA)>, frame: Res<Frame>) {
    let visit = |(value, mut out): (&Value, Mut<StageA>)| {
        record(&threads);
        out.0 = mix(value.value);
    };
    if frame.parallel {
        query.par_iter_mut().for_each(visit);
    } else {
        query.iter_mut().for_each(visit);
    }
}
fn b(threads: Res<Threads>, mut query: Query<(&StageA, &mut StageB)>, frame: Res<Frame>) {
    let visit = |(a, mut b): (&StageA, Mut<StageB>)| {
        record(&threads);
        b.0 = a.0.wrapping_mul(3);
    };
    if frame.parallel {
        query.par_iter_mut().for_each(visit);
    } else {
        query.iter_mut().for_each(visit);
    }
}
fn c(threads: Res<Threads>, mut query: Query<(&StageA, &mut StageC)>, frame: Res<Frame>) {
    let visit = |(a, mut c): (&StageA, Mut<StageC>)| {
        record(&threads);
        c.0 = !a.0;
    };
    if frame.parallel {
        query.par_iter_mut().for_each(visit);
    } else {
        query.iter_mut().for_each(visit);
    }
}
fn join(
    threads: Res<Threads>,
    mut query: Query<(&StageB, &StageC, &mut Value)>,
    frame: Res<Frame>,
) {
    let visit = |(b, c, mut value): (&StageB, &StageC, Mut<Value>)| {
        record(&threads);
        value.value = b.0.wrapping_add(c.0);
    };
    if frame.parallel {
        query.par_iter_mut().for_each(visit);
    } else {
        query.iter_mut().for_each(visit);
    }
}

pub fn observe(frame: Frame, scheduled: bool) -> serde_json::Value {
    SEEN.with(|seen| seen.set(false));
    let caller = format!("{:?}", std::thread::current().id());
    let (mut world, _) = setup(
        Case::Dag,
        Frame {
            count: 1003,
            ..frame
        },
    );
    world.insert_resource(Threads::default());
    if scheduled {
        let mut schedule = configured_schedule(frame.parallel);
        if frame.fork_join {
            schedule.add_systems((a.before(b).before(c), b.before(join), c.before(join), join));
        } else {
            schedule.add_systems((a, b, c, join).chain());
        }
        schedule.run(&mut world);
    } else {
        let mut query = world.query::<&mut Value>();
        let observer = Threads::default();
        let visit = |mut value: Mut<Value>| {
            record(&observer);
            value.value = mix(value.value);
        };
        if frame.parallel {
            query.par_iter_mut(&mut world).for_each(visit);
        } else {
            query.iter_mut(&mut world).for_each(visit);
        }
        world.insert_resource(observer);
    }
    let threads = world
        .resource::<Threads>()
        .0
        .lock()
        .expect("diagnostic observer has no panicking writers");
    serde_json::json!({"kind": "separate_1003_entity_scheduler_probe", "caller_thread_id": caller,
        "observed_thread_ids": *threads, "caller_observed": threads.contains(&caller),
        "observed_unique_runners": threads.len(), "proves_timed_case_concurrency": false,
        "instrumentation": "one mutex registration per participating thread; diagnostic only"})
}
