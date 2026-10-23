#![allow(clippy::unwrap_used)]
mod common;
use ara_anim::{
    glam::{Vec3, Vec4},
    graph::*,
    ik::*,
    matching::*,
    *,
};
use ara_schema::AssetRef;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
#[test]
fn procedural_ik_matching_inertialization_and_snapshot_replay_allocate_nothing() {
    let s = common::skeleton(8);
    let raw = common::clip(&s, 1.0);
    let clip = CookedClip::cook(&raw, &s, CompressionSettings::default())
        .unwrap()
        .0;
    let config = MatchConfig {
        bones: vec!["joint_7".into()],
        horizons: vec![16000],
        ..Default::default()
    };
    let db = MotionDatabase::cook(&config, &s, &[(common::reference(0), &clip)]).unwrap();
    let mut normalized = vec![Vec4::ZERO; db.query_groups()];
    let query = vec![0.0; config.dimensions()];
    db.normalize(&query, &mut normalized).unwrap();
    let r = AssetRef("@search.animation#an_db".into());
    let names: Vec<_> = (0..config.dimensions()).map(|i| format!("q{i}")).collect();
    let mut data = ControllerData {
        initial: "move".into(),
        ..Default::default()
    };
    data.parameters = names
        .iter()
        .map(|n| (n.clone(), Parameter::default()))
        .collect();
    data.nodes.insert(
        "search".into(),
        Node {
            kind: NodeKind::MotionMatching,
            clip: Some(r.clone()),
            matching: Some(MatchNode {
                query: names,
                continuation: 0,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    data.states.insert(
        "move".into(),
        State {
            node: "search".into(),
            ..Default::default()
        },
    );
    let c = Controller::compile_with_matching(
        &data,
        &s,
        vec![(common::reference(0), clip)],
        vec![(r, db)],
    )
    .unwrap();
    let mut actor = Animator::new(&c, &s).unwrap();
    let mut snapshot = actor.snapshot();
    let chain_data = ChainData {
        joints: vec![
            "joint_0".into(),
            "joint_1".into(),
            "joint_3".into(),
            "joint_7".into(),
        ],
        ..Default::default()
    };
    let mut chain = Chain::cook(&s, &chain_data).unwrap();
    let mut pose = s.bind_pose().clone();
    let region = Region::new(GLOBAL);
    for t in 0..90 {
        pose.copy_from(s.bind_pose()).unwrap();
        chain
            .solve(
                &s,
                &mut pose,
                Vec3::new(0.1, 0.3, 0.0),
                if t % 2 == 0 {
                    ChainSolver::Ccd
                } else {
                    ChainSolver::Fabrik
                },
                SolverSettings::default(),
            )
            .unwrap();
        actor.tick(&c, &s, |_| {}).unwrap();
        actor.snapshot_into(&mut snapshot);
        if t % 7 == 0 {
            actor.restore(&c, &s, &snapshot).unwrap();
        }
    }
    let stats = region.change();
    assert_eq!(
        (stats.allocations, stats.reallocations, stats.deallocations),
        (0, 0, 0),
        "{stats:?}"
    );
}
