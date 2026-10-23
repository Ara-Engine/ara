#![allow(clippy::unwrap_used)]
mod common;
use ara_anim::{
    glam::{Vec3, Vec3A, Vec4},
    graph::*,
    matching::*,
    *,
};
use ara_schema::AssetRef;

fn fixture() -> (Skeleton, Vec<CookedClip>, MatchConfig) {
    let skeleton = common::skeleton(8);
    let clips = [0.6, 1.8, -1.0]
        .iter()
        .map(|m| {
            CookedClip::cook(
                &common::clip(&skeleton, *m),
                &skeleton,
                CompressionSettings::default(),
            )
            .unwrap()
            .0
        })
        .collect();
    let config = MatchConfig {
        bones: vec!["joint_3".into(), "joint_7".into()],
        horizons: vec![8000, 16000, 24000],
        ..Default::default()
    };
    (skeleton, clips, config)
}
#[test]
fn normalized_simd_search_matches_scalar_exhaustive_and_respects_work_budget() {
    let (s, clips, config) = fixture();
    let borrowed: Vec<_> = clips
        .iter()
        .enumerate()
        .map(|(i, c)| (common::reference(i), c))
        .collect();
    let db = MotionDatabase::cook(&config, &s, &borrowed).unwrap();
    let mut bytes = db.to_bytes().unwrap();
    let loaded = MotionDatabase::from_bytes(&bytes, &s).unwrap();
    assert_eq!(loaded.len(), db.len());
    assert_eq!(loaded.normalized_row(17), db.normalized_row(17));
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert!(MotionDatabase::from_bytes(&bytes, &s).is_err());
    for query_index in (0..db.len()).step_by(7) {
        let mut query = db.normalized_row(query_index).unwrap().to_vec();
        for (i, q) in query.iter_mut().enumerate() {
            *q += Vec4::splat((i as f32 * 0.17).sin() * 0.03);
        }
        let result = db.search(&query, 0).unwrap();
        let scalar = (0..db.len())
            .map(|r| {
                let cost: f32 = db
                    .normalized_row(r)
                    .unwrap()
                    .iter()
                    .zip(&query)
                    .flat_map(|(a, b)| (*a - *b).to_array())
                    .map(|v| v * v)
                    .sum();
                (r, cost)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
            .unwrap();
        assert_eq!(result.row, scalar.0);
        assert!((result.cost - scalar.1).abs() < 0.001);
        let partial = db.search(&query, 17).unwrap();
        assert_eq!(partial.visited, 17);
        assert!(!partial.exact);
        assert!(partial.cost + 0.001 >= result.cost);
    }
}
#[test]
fn critically_damped_prediction_is_deterministic_and_inertialization_preserves_pose() {
    let mut predictor = TrajectoryPredictor::default();
    let mut samples = [TrajectoryPoint::default(); 3];
    predictor
        .predict(
            Vec3::new(2.0, 0.0, 0.0),
            0.6,
            0.08,
            &[0.2, 0.4, 0.8],
            1.0 / 60.0,
            &mut samples,
        )
        .unwrap();
    assert!(
        samples[2].position.x > samples[1].position.x
            && samples[1].position.x > samples[0].position.x
    );
    assert_eq!(predictor, TrajectoryPredictor::default());
    for _ in 0..120 {
        predictor
            .step(Vec3::X * 2.0, 0.6, 0.08, 1.0 / 60.0)
            .unwrap();
    }
    assert!(predictor.motion.value.distance(Vec3::X * 2.0) < 0.001);
    let mut outgoing = Pose::new(1);
    outgoing
        .set_joint(
            0,
            Transform {
                translation: Vec3A::X,
                rotation: glam::Quat::from_rotation_y(0.8),
                scale: Vec3A::splat(1.2),
            },
        )
        .unwrap();
    let mut incoming = Pose::new(1);
    let mut inertial = Inertializer::new(1, 0.05).unwrap();
    inertial
        .begin(&outgoing, &outgoing, &incoming, &incoming, 1.0 / 60.0)
        .unwrap();
    inertial.apply(&mut incoming, 0.0).unwrap();
    assert!(incoming.translations()[0].distance(outgoing.translations()[0]) < 1e-6);
    assert!(incoming.rotations()[0].angle_between(outgoing.rotations()[0]) < 0.001);
    for _ in 0..120 {
        incoming = Pose::new(1);
        inertial.apply(&mut incoming, 1.0 / 60.0).unwrap();
    }
    assert!(incoming.translations()[0].length() < 0.0001);
}
#[test]
fn motion_graph_replays_snapshots_curves_events_and_roots() {
    let (s, clips, config) = fixture();
    let borrowed: Vec<_> = clips
        .iter()
        .enumerate()
        .map(|(i, c)| (common::reference(i), c))
        .collect();
    let db = MotionDatabase::cook(&config, &s, &borrowed).unwrap();
    let database_ref = AssetRef("@locomotion.animation#an_search".into());
    let query: Vec<_> = (0..config.dimensions())
        .map(|i| format!("feature{i:02}"))
        .collect();
    let mut graph = ControllerData {
        initial: "move".into(),
        ..Default::default()
    };
    graph.parameters = query
        .iter()
        .map(|n| (n.clone(), Parameter::default()))
        .collect();
    graph.nodes.insert(
        "search".into(),
        Node {
            kind: NodeKind::MotionMatching,
            clip: Some(database_ref.clone()),
            matching: Some(MatchNode {
                query,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    graph.states.insert(
        "move".into(),
        State {
            node: "search".into(),
            ..Default::default()
        },
    );
    let controller = Controller::compile_with_matching(
        &graph,
        &s,
        clips
            .into_iter()
            .enumerate()
            .map(|(i, c)| (common::reference(i), c))
            .collect(),
        vec![(database_ref, db)],
    )
    .unwrap();
    let mut actor = Animator::new(&controller, &s).unwrap();
    for _ in 0..20 {
        actor.tick(&controller, &s, |_| {}).unwrap();
    }
    let snapshot = actor.snapshot();
    let mut roots = Vec::new();
    for _ in 0..10 {
        roots.push(actor.tick(&controller, &s, |_| {}).unwrap());
    }
    let expected = actor.pose().clone();
    let curves = actor.curves().to_vec();
    actor.restore(&controller, &s, &snapshot).unwrap();
    for root in roots {
        assert_eq!(root, actor.tick(&controller, &s, |_| {}).unwrap());
    }
    assert_eq!(&expected, actor.pose());
    assert_eq!(curves, actor.curves());
}
