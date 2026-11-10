//! Typed animation-controller edits shared by the editors: adding a state, blend or
//! transition, and connecting two canvas endpoints. Each is one undo step through
//! `editor.animation.controller.set`.
use crate::Editor;
use ara_anim::graph::{BlendSample, Node, NodeKind, Parameter, State, Transition};
use serde_json::json;

/// Adds to the controller `doc`: a `state` (with its clip node), a 1D `blend` state over the
/// `speed` parameter, or a `transition` between its first two states (one undo step).
pub(crate) fn edit(editor: &mut Editor, doc: &str, kind: &str) -> ara_ui::Result<()> {
    let doc = doc.to_owned();
    let (record, mut controller) = crate::authoring::animation::controller(
        editor
            .host
            .store()
            .workspace()
            .doc(&doc)
            .ok_or("controller missing")?,
    )?;
    let n = editor.host.store().revision();
    match kind {
        "state" => {
            let name = format!("State{}", controller.states.len() + 1);
            let pose = format!("clip{n}");
            controller.nodes.insert(pose.clone(), Node::default());
            controller.states.insert(
                name.clone(),
                State {
                    node: pose,
                    ..Default::default()
                },
            );
            if controller.initial.is_empty() {
                controller.initial = name;
            }
        }
        "blend" => {
            let mut children = controller.nodes.keys().take(2).cloned().collect::<Vec<_>>();
            while children.len() < 2 {
                let key = format!("clip{n}_{}", children.len());
                controller.nodes.insert(key.clone(), Node::default());
                children.push(key);
            }
            let name = format!("Locomotion{}", controller.nodes.len());
            controller
                .parameters
                .entry("speed".into())
                .or_insert_with(Parameter::default);
            controller.nodes.insert(
                name.clone(),
                Node {
                    kind: NodeKind::Blend1d,
                    parameter_x: "speed".into(),
                    samples: children
                        .into_iter()
                        .enumerate()
                        .map(|(i, node)| BlendSample {
                            node,
                            x: i as f32,
                            y: 0.,
                        })
                        .collect(),
                    ..Default::default()
                },
            );
            controller.states.insert(
                name.clone(),
                State {
                    node: name.clone(),
                    ..Default::default()
                },
            );
            if controller.initial.is_empty() {
                controller.initial = name;
            }
        }
        "transition" => {
            let states = controller
                .states
                .keys()
                .take(2)
                .cloned()
                .collect::<Vec<_>>();
            if let [from, to] = states.as_slice() {
                controller.transitions.push(Transition {
                    id: format!("transition{n}"),
                    from: from.clone(),
                    to: to.clone(),
                    duration: 4800,
                    exit_tick: None,
                    conditions: Vec::new(),
                    blend: None,
                });
            }
        }
        _ => {}
    }
    editor.apply_to(
        &doc,
        vec![json!({"cmd":"editor.animation.controller.set","args":{"record":record,"controller":controller}})],
        "Edit animation graph",
    )?;
    Ok(())
}

/// Connects two canvas endpoints of the controller `doc` shown as `graph`: a state to a
/// state adds a transition, a pose to a blend input sets that sample (one undo step).
pub(crate) fn connect(
    editor: &mut Editor,
    doc: &str,
    graph: &ara_graph::Graph,
    from: ara_graph::Endpoint,
    to: ara_graph::Endpoint,
) -> ara_ui::Result<()> {
    let (record, mut controller) = crate::authoring::animation::controller(
        editor
            .host
            .store()
            .workspace()
            .doc(doc)
            .ok_or("controller missing")?,
    )?;
    let source = graph.nodes.get(&from.node).ok_or("source node missing")?;
    let target = graph.nodes.get(&to.node).ok_or("target node missing")?;
    if let (Some(from), Some(to)) = (
        source.payload["state"].as_str(),
        target.payload["state"].as_str(),
    ) {
        controller.transitions.push(Transition {
            id: format!("transition{}", editor.host.store().revision()),
            from: from.into(),
            to: to.into(),
            duration: 4800,
            exit_tick: None,
            conditions: Vec::new(),
            blend: None,
        });
    } else if let (Some(from), Some(target)) = (
        source.payload["pose"].as_str(),
        target.payload["pose"].as_str(),
    ) {
        let node = controller
            .nodes
            .get_mut(target)
            .ok_or("pose node missing")?;
        let index = to
            .port
            .strip_prefix("sample")
            .and_then(|p| p.parse::<usize>().ok())
            .ok_or("invalid blend input")?;
        node.samples
            .get_mut(index)
            .ok_or("blend sample missing")?
            .node = from.into();
    }
    editor.apply_to(
        doc,
        vec![json!({"cmd":"editor.animation.controller.set","args":{"record":record,"controller":controller}})],
        "Connect animation graph",
    )?;
    Ok(())
}
