#![allow(clippy::unwrap_used)]
mod common;
use ara_anim::{glam::Mat4, graph::ParameterValue, *};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[test]
fn playback_crossfade_seek_snapshot_and_palette_allocate_nothing() {
    let skeleton = common::skeleton(65);
    let c = common::controller(&common::controller_data(), &skeleton);
    let mut actor = Animator::new(&c, &skeleton).unwrap();
    let mut snapshot = actor.snapshot();
    let mut model = vec![Mat4::IDENTITY; 65];
    let mut palette = model.clone();
    actor
        .set(c.parameter("go").unwrap(), ParameterValue::Trigger(true))
        .unwrap();
    let region = Region::new(GLOBAL);
    for tick in 0..120 {
        actor
            .tick(&c, &skeleton, |e| {
                std::hint::black_box(e);
            })
            .unwrap();
        actor.snapshot_into(&mut snapshot);
        if tick % 7 == 0 {
            actor.restore(&c, &skeleton, &snapshot).unwrap();
        }
        pose::model_matrices(&skeleton, actor.pose(), &mut model).unwrap();
        pose::skinning_palette(&skeleton, &model, &mut palette).unwrap();
    }
    let stats = region.change();
    assert_eq!(
        (stats.allocations, stats.reallocations, stats.deallocations),
        (0, 0, 0),
        "{stats:?}"
    );
}
