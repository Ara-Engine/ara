//! Jump and interaction clips extend the native locomotion controller via typed commands.
use super::*;
use ara_anim::graph::{
    Condition, ControllerData, Node, NodeKind, Parameter, ParameterKind, State, Transition,
};

pub(super) fn actions(ctx: &Ctx, path: &str, controller: &str) -> Result<()> {
    let doc: Value = serde_json::from_slice(&std::fs::read(ctx.path(path))?)?;
    let mut data: ControllerData =
        serde_json::from_value(doc["records"][controller]["controller"].clone())?;
    let mut calls = Vec::new();
    for (parameter, clip, record, duration) in [
        ("jump", "Jump", "an_jump", 24_000),
        ("interact", "Punch", "an_interact", 19_200),
    ] {
        calls.push((
            "animation.create",
            json!({"id":record,"asset":{"name":clip}}),
        ));
        data.parameters.insert(
            parameter.into(),
            Parameter {
                kind: ParameterKind::Bool,
                ..Default::default()
            },
        );
        data.nodes.insert(
            parameter.into(),
            Node {
                kind: NodeKind::Clip,
                clip: Some(ara_schema::AssetRef(format!("@{path}#{record}"))),
                ..Default::default()
            },
        );
        data.states.insert(
            parameter.into(),
            State {
                node: parameter.into(),
                duration,
                looping: false,
                ..Default::default()
            },
        );
        for from in ["idle", "move"] {
            data.transitions.insert(
                0,
                Transition {
                    id: format!("{from}-{parameter}"),
                    from: from.into(),
                    to: parameter.into(),
                    duration: 3200,
                    conditions: vec![Condition {
                        parameter: parameter.into(),
                        bool_value: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            );
        }
        data.transitions.push(Transition {
            id: format!("{parameter}-done"),
            from: parameter.into(),
            to: "idle".into(),
            duration: 6400,
            exit_tick: Some(duration),
            ..Default::default()
        });
    }
    calls.push((
        "animation.controller.set",
        json!({"id":controller,"value":data}),
    ));
    ctx.apply(
        path,
        "Blend jump and interaction feedback into locomotion",
        calls,
    )
}
