#![allow(clippy::unwrap_used)]
extern crate glam34 as glam;
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
    let empty = CookedClip::cook(
        &data::ClipData {
            duration: 48000,
            tracks: vec![data::JointTrack::default(); 65],
            ..Default::default()
        },
        &skeleton,
        CompressionSettings::default(),
    )
    .unwrap()
    .0;
    let mut source = common::clip(&skeleton, 1.0);
    source.curves.clear();
    let clip = CookedClip::cook(&source, &skeleton, CompressionSettings::default())
        .unwrap()
        .0;
    let mut sampler = Sampler::new(&empty);
    let mut sampled = Pose::new(65);
    actor
        .set(c.parameter("go").unwrap(), ParameterValue::Trigger(true))
        .unwrap();
    let region = Region::new(GLOBAL);
    for tick in 0..120 {
        let asset = if tick % 2 == 0 { &empty } else { &clip };
        sampler
            .sample_prefix(
                asset,
                &skeleton,
                tick * 317,
                tick as usize % 66,
                &mut sampled,
                &mut [],
            )
            .unwrap();
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
