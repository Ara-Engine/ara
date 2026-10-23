#![allow(dead_code)]
use ara_anim::{
    CompressionSettings, Controller, CookedClip, Skeleton, data::*, glam::Quat, graph::*,
};
use ara_schema::AssetRef;

pub fn skeleton(count: usize) -> Skeleton {
    Skeleton::cook(SkeletonData {
        joints: (0..count)
            .map(|i| Joint {
                id: format!("joint_{i}"),
                name: format!("Joint {i}"),
                parent: if i == 0 {
                    None
                } else {
                    Some(((i - 1) / 2) as u32)
                },
                bind: AnimTransform {
                    translation: if i == 0 { [0.0; 3] } else { [0.0, 0.2, 0.0] },
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect(),
        motion_root: 0,
        ..Default::default()
    })
    .expect("generated skeleton is valid")
}
pub fn clip(skeleton: &Skeleton, multiplier: f32) -> ClipData {
    ClipData {
        duration: 48000,
        tracks: (0..skeleton.len())
            .map(|bone| {
                let phase = bone as f32 * 0.1;
                JointTrack {
                    translations: (0..=60)
                        .map(|frame| {
                            let t = frame as f32 / 60.0;
                            VectorKey {
                                tick: frame * 800,
                                value: if bone == 0 {
                                    [t * multiplier, 0.0, 0.0]
                                } else {
                                    [
                                        0.01 * (t * std::f32::consts::TAU + phase).sin()
                                            * multiplier,
                                        0.2,
                                        0.0,
                                    ]
                                },
                            }
                        })
                        .collect(),
                    rotations: (0..=60)
                        .map(|frame| RotationKey {
                            tick: frame * 800,
                            value: Quat::from_rotation_z(
                                (frame as f32 / 60.0 * std::f32::consts::TAU + phase).sin()
                                    * 0.3
                                    * multiplier,
                            )
                            .to_array(),
                        })
                        .collect(),
                    scales: vec![VectorKey {
                        tick: 0,
                        value: [1.0; 3],
                    }],
                    ..Default::default()
                }
            })
            .collect(),
        curves: vec![Curve {
            property: "MorphWeights.smile".into(),
            keys: vec![
                ScalarKey {
                    tick: 0,
                    value: 0.0,
                },
                ScalarKey {
                    tick: 48000,
                    value: 1.0,
                },
            ],
            ..Default::default()
        }],
        notifies: vec![
            Notify {
                id: "step_a".into(),
                name: "Footstep".into(),
                tick: 12000,
                payload: "left".into(),
            },
            Notify {
                id: "step_b".into(),
                name: "Footstep".into(),
                tick: 36000,
                payload: "right".into(),
            },
        ],
        ..Default::default()
    }
}
pub fn reference(index: usize) -> AssetRef {
    AssetRef(format!("@animations/clip{index}.animation#ac_{index:08}"))
}
pub fn clip_node(index: usize) -> Node {
    Node {
        clip: Some(reference(index)),
        ..Default::default()
    }
}
pub fn sample(node: &str, x: f32, y: f32) -> BlendSample {
    BlendSample {
        node: node.into(),
        x,
        y,
    }
}
pub fn controller_data() -> ControllerData {
    ControllerData {
        parameters: [
            ("speed".into(), Parameter::default()),
            (
                "go".into(),
                Parameter {
                    kind: ParameterKind::Trigger,
                    ..Default::default()
                },
            ),
        ]
        .into(),
        nodes: [
            ("idle".into(), clip_node(0)),
            ("run".into(), clip_node(1)),
            (
                "locomotion".into(),
                Node {
                    kind: NodeKind::Blend1d,
                    parameter_x: "speed".into(),
                    samples: vec![sample("idle", 0.0, 0.0), sample("run", 1.0, 0.0)],
                    ..Default::default()
                },
            ),
        ]
        .into(),
        states: [
            (
                "idle".into(),
                State {
                    node: "idle".into(),
                    sync_group: "feet".into(),
                    ..Default::default()
                },
            ),
            (
                "move".into(),
                State {
                    node: "locomotion".into(),
                    sync_group: "feet".into(),
                    ..Default::default()
                },
            ),
        ]
        .into(),
        transitions: vec![Transition {
            id: "start".into(),
            from: "idle".into(),
            to: "move".into(),
            duration: 4800,
            conditions: vec![Condition {
                parameter: "go".into(),
                comparison: Comparison::Trigger,
                ..Default::default()
            }],
            ..Default::default()
        }],
        initial: "idle".into(),
        ..Default::default()
    }
}
pub fn controller(data: &ControllerData, s: &Skeleton) -> Controller {
    let bindings = (0..2)
        .map(|i| {
            (
                reference(i),
                CookedClip::cook(&clip(s, i as f32 + 1.0), s, CompressionSettings::default())
                    .expect("clip cooks")
                    .0,
            )
        })
        .collect();
    Controller::compile(data, s, bindings).expect("controller compiles")
}
pub fn near(a: f32, b: f32, tolerance: f32) {
    assert!(
        (a - b).abs() <= tolerance,
        "{a} != {b}, tolerance {tolerance}"
    );
}
