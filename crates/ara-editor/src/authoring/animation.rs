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
/// Clip ticks per second. Controllers share their clips' timebase; the editor reads
/// times against the default clip frequency (48 kHz) and shows them in seconds.
pub(crate) fn ticks_per_second() -> u32 {
    ara_anim::data::ClipData::default().ticks_per_second
}

/// Clip ticks as people read them: "1 s", "0.1 s", "1.25 s".
pub(crate) fn seconds(ticks: u32) -> String {
    format!("{} s", seconds_value(ticks))
}

/// Clip ticks as a bare seconds number for an editable field ("0.1").
pub(crate) fn seconds_value(ticks: u32) -> String {
    let text = format!("{:.3}", f64::from(ticks) / f64::from(ticks_per_second()));
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A person's time ("0.1", "0.1 s", "100 ms", or a number) as clip ticks; blank is `None`.
pub(crate) fn ticks(value: &serde_json::Value) -> Result<Option<u32>, String> {
    let seconds = match value {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Number(n) => n.as_f64().ok_or("time must be a number")?,
        serde_json::Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            if let Some(ms) = text.strip_suffix("ms") {
                ms.trim()
                    .parse::<f64>()
                    .map_err(|_| "time must be a number")?
                    / 1000.
            } else {
                text.trim_end_matches('s')
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "time must be a number of seconds")?
            }
        }
        _ => return Err("time must be a number of seconds".into()),
    };
    if !seconds.is_finite() || seconds < 0. {
        return Err("time must be zero or more seconds".into());
    }
    Ok(Some(
        (seconds * f64::from(ticks_per_second())).round() as u32
    ))
}

/// The canvas edge id of a transition (stable across projections).
pub(crate) fn transition_edge(transition: &str) -> String {
    id("w", transition)
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
                    payload: json!({"name":key,"pose":key,"samples":node.samples.iter().map(|s|json!({"node":s.node,"x":s.x,"y":s.y})).collect::<Vec<_>>()}),
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
        // Condition summaries ride on the header for the canvas's arrow labels.
        graph.header.payload["transitions"] = controller
            .transitions
            .iter()
            .map(|t| (id("w", &t.id), json!(conditions(t, &controller.parameters))))
            .collect::<serde_json::Map<_, _>>()
            .into();
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

/// A transition's conditions as people read them: "speed > 0.2 and grounded",
/// "jump" for a trigger, "after 4800 ticks" for an exit time, else "always".
pub(crate) fn conditions(
    transition: &ara_anim::graph::Transition,
    parameters: &BTreeMap<String, ara_anim::graph::Parameter>,
) -> String {
    use ara_anim::graph::{Comparison, ParameterKind};
    let parts = transition
        .conditions
        .iter()
        .map(|c| {
            let kind = parameters
                .get(&c.parameter)
                .map(|p| p.kind)
                .unwrap_or_default();
            let value = match kind {
                ParameterKind::Int => c.int_value.to_string(),
                ParameterKind::Bool => c.bool_value.to_string(),
                _ => {
                    let text = format!("{:.3}", c.float_value);
                    text.trim_end_matches('0').trim_end_matches('.').to_owned()
                }
            };
            match (kind, c.comparison) {
                (_, Comparison::Trigger) | (ParameterKind::Trigger, _) => c.parameter.clone(),
                (ParameterKind::Bool, Comparison::Equal) if c.bool_value => c.parameter.clone(),
                (ParameterKind::Bool, Comparison::NotEqual) if !c.bool_value => c.parameter.clone(),
                (ParameterKind::Bool, _) => format!("not {}", c.parameter),
                (_, Comparison::Equal) => format!("{} = {value}", c.parameter),
                (_, Comparison::NotEqual) => format!("{} ≠ {value}", c.parameter),
                (_, Comparison::Greater) => format!("{} > {value}", c.parameter),
                (_, Comparison::Less) => format!("{} < {value}", c.parameter),
            }
        })
        .collect::<Vec<_>>();
    if !parts.is_empty() {
        parts.join(" and ")
    } else if let Some(tick) = transition.exit_tick {
        format!("after {}", seconds(tick))
    } else {
        "always".into()
    }
}

/// Authoring permits unassigned clips but never cyclic pose evaluation.
pub(crate) fn validate(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_in_seconds_and_type_back_to_ticks() {
        assert_eq!(seconds(48_000), "1 s");
        assert_eq!(seconds(4_800), "0.1 s");
        assert_eq!(seconds_value(60_000), "1.25");
        assert_eq!(ticks(&json!(0.1)), Ok(Some(4_800)));
        assert_eq!(ticks(&json!("0.25 s")), Ok(Some(12_000)));
        assert_eq!(ticks(&json!("100 ms")), Ok(Some(4_800)));
        assert_eq!(ticks(&json!("")), Ok(None));
        assert!(ticks(&json!(-1)).is_err());
    }
}
