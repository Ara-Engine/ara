#![allow(clippy::unwrap_used)]
mod common;
use ara_anim::{data::*, graph::*, *};
use serde_json::Value;

fn snapshot_json(a: &Animator) -> Value {
    serde_json::to_value(a.snapshot()).unwrap()
}
fn constant_program(mut data: ControllerData, s: &Skeleton, values: &[f32]) -> Controller {
    data.extract_root_motion = false;
    let bindings = values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let clip = ClipData {
                duration: 48000,
                tracks: (0..s.len())
                    .map(|_| JointTrack {
                        translations: vec![VectorKey {
                            tick: 0,
                            value: [*value, 0.0, 0.0],
                        }],
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            };
            (
                common::reference(i),
                CookedClip::cook(&clip, s, CompressionSettings::default())
                    .unwrap()
                    .0,
            )
        })
        .collect();
    Controller::compile(&data, s, bindings).unwrap()
}

#[test]
fn transitions_crossfade_consume_triggers_and_sync_different_durations() {
    let s = common::skeleton(2);
    let mut data = common::controller_data();
    data.states.get_mut("move").unwrap().duration = 24000;
    let c = common::controller(&data, &s);
    let mut a = Animator::new(&c, &s).unwrap();
    for _ in 0..15 {
        a.tick(&c, &s, |_| {}).unwrap();
    }
    a.set(c.parameter("go").unwrap(), ParameterValue::Trigger(true))
        .unwrap();
    a.set(c.parameter("speed").unwrap(), ParameterValue::Float(0.75))
        .unwrap();
    assert!(
        a.set(c.parameter("speed").unwrap(), ParameterValue::Bool(true))
            .is_err()
    );
    a.tick(&c, &s, |_| {}).unwrap();
    assert_eq!(
        a.get(c.parameter("go").unwrap()),
        Some(ParameterValue::Trigger(false))
    );
    let snap = snapshot_json(&a);
    assert_eq!(snap["fade"]["elapsed"], 800);
    assert_eq!(snap["fade"]["target"]["time"], 6400); // Preserve 12800/48000 phase throughout the fade.
    for _ in 0..5 {
        a.tick(&c, &s, |_| {}).unwrap();
    }
    assert!(snapshot_json(&a)["fade"].is_null());
    assert_eq!(
        a.pose().joint(0).unwrap().translation,
        s.bind_pose().joint(0).unwrap().translation
    );
}

#[test]
fn rollback_replays_pose_root_parameters_and_event_identity_bit_for_bit() {
    let s = common::skeleton(5);
    let mut data = common::controller_data();
    data.states.get_mut("move").unwrap().duration = 24017;
    let c = common::controller(&data, &s);
    let mut a = Animator::new(&c, &s).unwrap();
    for _ in 0..17 {
        a.tick(&c, &s, |_| {}).unwrap();
    }
    a.set(c.parameter("go").unwrap(), ParameterValue::Trigger(true))
        .unwrap();
    a.set(c.parameter("speed").unwrap(), ParameterValue::Float(0.3))
        .unwrap();
    a.tick(&c, &s, |_| {}).unwrap();
    assert_ne!(snapshot_json(&a)["fade"]["phase_remainder"], 0);
    let mut saved = a.snapshot();
    a.snapshot_into(&mut saved);
    let encoded = serde_json::to_vec(&saved).unwrap();
    let saved: Snapshot = serde_json::from_slice(&encoded).unwrap();
    let mut expected = Vec::new();
    for _ in 0..180 {
        let mut events = Vec::new();
        let root = a
            .tick(&c, &s, |e| {
                events.push((
                    e.tick,
                    e.lane,
                    e.clip,
                    e.notify.id.clone(),
                    e.cycle,
                    e.weight,
                ))
            })
            .unwrap();
        expected.push((
            a.pose().clone(),
            a.curves().to_vec(),
            root,
            events,
            a.snapshot(),
        ));
    }
    a.restore(&c, &s, &saved).unwrap();
    for (pose, curves, root, events, snapshot) in expected {
        let mut replay_events = Vec::new();
        let replay_root = a
            .tick(&c, &s, |e| {
                replay_events.push((
                    e.tick,
                    e.lane,
                    e.clip,
                    e.notify.id.clone(),
                    e.cycle,
                    e.weight,
                ))
            })
            .unwrap();
        assert_eq!(
            (
                a.pose(),
                a.curves(),
                replay_root,
                replay_events,
                a.snapshot()
            ),
            (&pose, curves.as_slice(), root, events, snapshot)
        );
    }
    let mut wrong = serde_json::to_value(&saved).unwrap();
    wrong["lane"]["state"] = 1000.into();
    let wrong: Snapshot = serde_json::from_value(wrong).unwrap();
    let before = a.snapshot();
    assert!(a.restore(&c, &s, &wrong).is_err());
    assert_eq!(a.snapshot(), before);
}

#[test]
fn one_and_two_dimensional_blends_hit_samples_interiors_and_clamped_rays() {
    let s = common::skeleton(2);
    let mut data = common::controller_data();
    data.states.get_mut("idle").unwrap().node = "locomotion".into();
    data.transitions.clear();
    let c = constant_program(data.clone(), &s, &[0.0, 10.0]);
    let mut a = Animator::new(&c, &s).unwrap();
    for (x, expected) in [(-1.0, 0.0), (0.25, 2.5), (1.0, 10.0), (100.0, 10.0)] {
        a.set(c.parameter("speed").unwrap(), ParameterValue::Float(x))
            .unwrap();
        a.tick(&c, &s, |_| {}).unwrap();
        common::near(a.pose().translations()[1].x, expected, 1e-5);
    }
    data.parameters
        .insert("vertical".into(), Parameter::default());
    for i in 2..5 {
        data.nodes.insert(format!("clip{i}"), common::clip_node(i));
    }
    data.nodes.insert(
        "locomotion".into(),
        Node {
            kind: NodeKind::Directional2d,
            parameter_x: "speed".into(),
            parameter_y: "vertical".into(),
            samples: vec![
                common::sample("idle", 0.0, 0.0),
                common::sample("run", 1.0, 0.0),
                common::sample("clip2", 0.0, 1.0),
                common::sample("clip3", -1.0, 0.0),
                common::sample("clip4", 0.0, -1.0),
            ],
            ..Default::default()
        },
    );
    let c = constant_program(data, &s, &[0.0, 10.0, 20.0, 30.0, 40.0]);
    let mut a = Animator::new(&c, &s).unwrap();
    for (x, y, expected) in [
        (0.0, 0.0, 0.0),
        (1.0, 0.0, 10.0),
        (0.0, 1.0, 20.0),
        (0.25, 0.25, 7.5),
        (2.0, 2.0, 15.0),
        (-1.0, 0.0, 30.0),
        (0.0, -1.0, 40.0),
    ] {
        a.set(c.parameter("speed").unwrap(), ParameterValue::Float(x))
            .unwrap();
        a.set(c.parameter("vertical").unwrap(), ParameterValue::Float(y))
            .unwrap();
        a.tick(&c, &s, |_| {}).unwrap();
        common::near(a.pose().translations()[1].x, expected, 1e-4);
    }
}

#[test]
fn graph_layers_and_additive_respect_bone_masks() {
    let s = common::skeleton(2);
    for (kind, expected) in [(NodeKind::Layer, 5.0), (NodeKind::Additive, 6.0)] {
        let mut data = common::controller_data();
        data.states.get_mut("idle").unwrap().node = "locomotion".into();
        data.nodes.insert(
            "locomotion".into(),
            Node {
                kind,
                samples: vec![
                    common::sample("idle", 0.0, 0.0),
                    common::sample("run", 0.0, 0.0),
                ],
                weight: 0.5,
                bone_mask: vec![0.0, 1.0],
                ..Default::default()
            },
        );
        let c = constant_program(data, &s, &[2.0, 8.0]);
        let mut a = Animator::new(&c, &s).unwrap();
        a.tick(&c, &s, |_| {}).unwrap();
        common::near(a.pose().translations()[0].x, 2.0, 1e-5);
        common::near(a.pose().translations()[1].x, expected, 1e-5);
    }
}

#[test]
fn compiler_rejects_cycles_missing_bindings_and_wrong_condition_types() {
    let s = common::skeleton(1);
    let bindings = || {
        (0..2)
            .map(|i| {
                (
                    common::reference(i),
                    CookedClip::cook(&common::clip(&s, 1.0), &s, CompressionSettings::default())
                        .unwrap()
                        .0,
                )
            })
            .collect()
    };
    let mut data = common::controller_data();
    data.nodes.get_mut("locomotion").unwrap().samples[0].node = "locomotion".into();
    assert!(Controller::compile(&data, &s, bindings()).is_err());
    data = common::controller_data();
    data.transitions[0].conditions[0].parameter = "speed".into();
    assert!(Controller::compile(&data, &s, bindings()).is_err());
    assert!(Controller::compile(&common::controller_data(), &s, Vec::new()).is_err());
}

#[test]
fn notifies_cover_multiwrap_boundaries_once_and_dag_paths_accumulate_weights() {
    let s = common::skeleton(1);
    let mut source = ClipData {
        duration: 100,
        tracks: vec![JointTrack::default()],
        notifies: vec![
            Notify {
                id: "zero".into(),
                tick: 0,
                ..Default::default()
            },
            Notify {
                id: "middle".into(),
                tick: 50,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let make = |source: &ClipData, looping| {
        let mut data = common::controller_data();
        data.fixed_step = 250;
        data.nodes.clear();
        data.nodes.insert("leaf".into(), common::clip_node(0));
        data.nodes.insert(
            "shared".into(),
            Node {
                kind: NodeKind::Blend1d,
                parameter_x: "speed".into(),
                samples: vec![
                    common::sample("leaf", 0.0, 0.0),
                    common::sample("leaf", 1.0, 0.0),
                ],
                ..Default::default()
            },
        );
        data.states = [(
            "only".into(),
            State {
                node: "shared".into(),
                looping,
                ..Default::default()
            },
        )]
        .into();
        data.initial = "only".into();
        data.transitions.clear();
        Controller::compile(
            &data,
            &s,
            vec![(
                common::reference(0),
                CookedClip::cook(source, &s, CompressionSettings::default())
                    .unwrap()
                    .0,
            )],
        )
        .unwrap()
    };
    let c = make(&source, true);
    let mut a = Animator::new(&c, &s).unwrap();
    a.set(c.parameter("speed").unwrap(), ParameterValue::Float(0.5))
        .unwrap();
    let mut events = Vec::new();
    a.tick(&c, &s, |e| {
        events.push((e.notify.id.as_str(), e.cycle, e.weight))
    })
    .unwrap();
    assert_eq!(
        events,
        [
            ("middle", 0, 1.0),
            ("zero", 1, 1.0),
            ("middle", 1, 1.0),
            ("zero", 2, 1.0),
            ("middle", 2, 1.0)
        ]
    );
    events.clear();
    a.tick(&c, &s, |e| {
        events.push((e.notify.id.as_str(), e.cycle, e.weight))
    })
    .unwrap();
    assert_eq!(events.first(), Some(&("zero", 3, 1.0)));
    source.notifies.push(Notify {
        id: "duplicate".into(),
        tick: 50,
        ..Default::default()
    });
    let c = make(&source, false);
    let mut a = Animator::new(&c, &s).unwrap();
    let mut events = Vec::new();
    a.tick(&c, &s, |e| events.push(e.notify.id.as_str()))
        .unwrap();
    a.tick(&c, &s, |e| events.push(e.notify.id.as_str()))
        .unwrap();
    assert_eq!(events, ["middle", "duplicate"]);
}

#[test]
fn ordered_numeric_boolean_conditions_and_exit_time_do_not_lose_triggers() {
    let s = common::skeleton(1);
    let mut data = common::controller_data();
    data.parameters.insert(
        "enabled".into(),
        Parameter {
            kind: ParameterKind::Bool,
            ..Default::default()
        },
    );
    data.parameters.insert(
        "count".into(),
        Parameter {
            kind: ParameterKind::Int,
            ..Default::default()
        },
    );
    data.transitions[0].duration = 0;
    data.transitions[0].exit_tick = Some(48000);
    data.transitions[0].conditions.extend([
        Condition {
            parameter: "enabled".into(),
            bool_value: true,
            ..Default::default()
        },
        Condition {
            parameter: "count".into(),
            comparison: Comparison::Greater,
            int_value: 2,
            ..Default::default()
        },
        Condition {
            parameter: "speed".into(),
            comparison: Comparison::Less,
            float_value: 0.5,
            ..Default::default()
        },
    ]);
    let c = common::controller(&data, &s);
    let mut a = Animator::new(&c, &s).unwrap();
    a.set(c.parameter("go").unwrap(), ParameterValue::Trigger(true))
        .unwrap();
    a.set(c.parameter("enabled").unwrap(), ParameterValue::Bool(true))
        .unwrap();
    a.set(c.parameter("count").unwrap(), ParameterValue::Int(3))
        .unwrap();
    for _ in 0..60 {
        a.tick(&c, &s, |_| {}).unwrap();
    }
    assert_eq!(
        a.get(c.parameter("go").unwrap()),
        Some(ParameterValue::Trigger(true))
    );
    a.tick(&c, &s, |_| {}).unwrap();
    assert_eq!(
        a.get(c.parameter("go").unwrap()),
        Some(ParameterValue::Trigger(false))
    );
    assert_eq!(snapshot_json(&a)["lane"]["state"], 1);
}
