//! Read-only controller projection; the animation document remains the sole program.
use ara_anim::graph::ControllerData;
use ara_graph::{Direction, Edge, Endpoint, Graph, Node, Port};
use serde_json::json;
use std::collections::BTreeMap;
pub(crate) fn controller(doc: &ara_doc::Document) -> Result<(String, ControllerData), String> {
    let (id, record) = doc
        .records()
        .iter()
        .find(|(_, r)| r.get("controller").is_some_and(|v| !v.is_null()))
        .ok_or("animation controller missing")?;
    let data = serde_json::from_value(
        record
            .get("controller")
            .cloned()
            .ok_or("controller missing")?,
    )
    .map_err(|e| e.to_string())?;
    Ok((id.to_string(), data))
}
pub(crate) fn layout_path(doc: &str) -> String {
    format!(".ara/editor/animation-{doc}.graph")
}
fn id(prefix: &str, key: &str) -> String {
    let hash = ara_asset::Digest::of(key.as_bytes()).to_string();
    format!("{prefix}_{}", &hash[..20])
}
fn port(ty: &str, direction: Direction) -> Port {
    Port {
        ty: ty.into(),
        direction,
        doc: format!("Typed {ty} connection"),
        required: false,
    }
}
pub(crate) fn projection(
    doc: &ara_doc::Document,
    layout: ara_graph::Editor,
    blends: bool,
) -> Result<Graph, String> {
    let (_, controller) = controller(doc)?;
    let mut graph = Graph::new(doc.id.clone(), "animation");
    graph.editor = layout;
    graph.header.payload = json!({"blends":blends});
    if blends {
        for (index, (key, node)) in controller.nodes.iter().enumerate() {
            let node_id = id("n", key);
            let mut ports = BTreeMap::from([("pose".into(), port("pose", Direction::Output))]);
            for (n, _) in node.samples.iter().enumerate() {
                ports.insert(format!("sample{n}"), port("pose", Direction::Input));
            }
            graph.nodes.insert(
                node_id.clone(),
                Node {
                    kind: format!("animation.{:?}", node.kind),
                    version: 1,
                    ports,
                    params: Default::default(),
                    payload: json!({"name":key,"pose":key}),
                    subgraph: None,
                },
            );
            graph
                .editor
                .positions
                .entry(node_id.clone())
                .or_insert(if node.samples.is_empty() {
                    [0., index as f32 * 150.]
                } else {
                    [320., 100.]
                });
            for (n, sample) in node.samples.iter().enumerate() {
                graph.edges.insert(
                    id("w", &format!("{key}/{n}")),
                    Edge {
                        from: Endpoint {
                            node: id("n", &sample.node),
                            port: "pose".into(),
                        },
                        to: Endpoint {
                            node: node_id.clone(),
                            port: format!("sample{n}"),
                        },
                    },
                );
            }
        }
    } else {
        for (index, (key, state)) in controller.states.iter().enumerate() {
            let node_id = id("n", key);
            graph.nodes.insert(
                node_id.clone(),
                Node {
                    kind: if key == &controller.initial {
                        "animation.entry".into()
                    } else {
                        "animation.state".into()
                    },
                    version: 1,
                    ports: BTreeMap::from([
                        ("in".into(), port("state", Direction::Input)),
                        ("out".into(), port("state", Direction::Output)),
                    ]),
                    params: Default::default(),
                    payload: json!({"name":key,"state":key,"pose":state.node}),
                    subgraph: None,
                },
            );
            graph.editor.positions.entry(node_id).or_insert([
                if index % 2 == 0 { 0. } else { 320. },
                (index / 2) as f32 * 220.,
            ]);
        }
        for transition in &controller.transitions {
            graph.edges.insert(
                id("w", &transition.id),
                Edge {
                    from: Endpoint {
                        node: id("n", &transition.from),
                        port: "out".into(),
                    },
                    to: Endpoint {
                        node: id("n", &transition.to),
                        port: "in".into(),
                    },
                },
            );
        }
    }
    Ok(graph)
}

/// Authoring permits unassigned clips but never cyclic pose evaluation.
pub(super) fn validate(
    controller: &ara_anim::graph::ControllerData,
) -> Result<(), ara_diag::Diagnostic> {
    if !controller.states.is_empty() && !controller.states.contains_key(&controller.initial) {
        return Err(crate::invalid("Initial state does not exist"));
    }
    let mut complete = std::collections::BTreeSet::new();
    fn visit<'a>(
        key: &'a str,
        controller: &'a ara_anim::graph::ControllerData,
        active: &mut std::collections::BTreeSet<&'a str>,
        complete: &mut std::collections::BTreeSet<&'a str>,
    ) -> Result<(), ara_diag::Diagnostic> {
        if complete.contains(key) {
            return Ok(());
        }
        if !active.insert(key) {
            return Err(crate::invalid(
                "Animation pose connections would form a cycle",
            ));
        }
        let node = controller
            .nodes
            .get(key)
            .ok_or_else(|| crate::invalid("Pose node is missing"))?;
        for sample in &node.samples {
            visit(&sample.node, controller, active, complete)?;
        }
        active.remove(key);
        complete.insert(key);
        Ok(())
    }
    for key in controller.nodes.keys() {
        visit(key, controller, &mut Default::default(), &mut complete)?;
    }
    Ok(())
}
