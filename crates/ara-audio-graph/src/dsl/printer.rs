//! Canonical text: parameters in declaration order, nodes in dependency order (author
//! order breaks ties), positional arguments, then outputs. Never prints IDs.

use crate::ast::{AudioGraph, Node, Output, ParamKind, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

fn number(v: f32) -> String {
    if v == 0.0 { "0".into() } else { format!("{v}") }
}

fn string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn value(graph: &AudioGraph, v: &Value) -> String {
    match v {
        Value::Number { value } => number(*value),
        Value::Clip { reference } => reference.clone(),
        Value::Ref { node, port } => {
            let name = graph.name_of(node).unwrap_or(node);
            // Multi-output nodes always name the port (`player.out`, `stereo.left`).
            let outputs = graph.nodes.get(node).map(|n| n.op.spec().outputs);
            let port = port
                .as_deref()
                .or_else(|| outputs.filter(|o| o.len() > 1).map(|o| o[0].name));
            match port {
                Some(p) => format!("{name}.{p}"),
                None => name.to_owned(),
            }
        }
    }
}

fn call(graph: &AudioGraph, node: &Node) -> String {
    let spec = node.op.spec();
    let variadic = (0..spec.variadic.map_or(0, |v| v.max))
        .take_while(|i| node.args.contains_key(&format!("in{i}")))
        .count();
    let fixed = if variadic > 0 {
        spec.inputs.len()
    } else {
        spec.inputs
            .iter()
            .rposition(|i| node.args.contains_key(i.name))
            .map_or(0, |p| p + 1)
    };
    let mut args = Vec::with_capacity(fixed + variadic);
    for input in &spec.inputs[..fixed] {
        args.push(match node.args.get(input.name) {
            Some(v) => value(graph, v),
            None => match input.default {
                // NaN (`UNCONNECTED`) marks an optional input with no literal default.
                Some(d) if !d.is_nan() => number(d),
                _ => "_".into(),
            },
        });
    }
    for i in 0..variadic {
        if let Some(v) = node.args.get(&format!("in{i}")) {
            args.push(value(graph, v));
        }
    }
    format!("{}({})", spec.name, args.join(", "))
}

/// Node IDs in dependency order; ready nodes leave in author order.
pub(crate) fn dependency_order(graph: &AudioGraph) -> Vec<&str> {
    let mut waiting: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (id, node) in &graph.nodes {
        let deps = node
            .args
            .values()
            .filter_map(|v| match v {
                Value::Ref { node, .. } if graph.nodes.contains_key(node) && node != id => {
                    Some(node.as_str())
                }
                _ => None,
            })
            .collect();
        waiting.insert(id, deps);
    }
    let mut out = Vec::with_capacity(waiting.len());
    while !waiting.is_empty() {
        let next = waiting
            .iter()
            .filter(|(_, deps)| deps.is_empty())
            .map(|(id, _)| *id)
            .min_by_key(|id| (graph.nodes[*id].order, *id))
            // A cycle: emit the earliest remaining node so text stays printable.
            .or_else(|| {
                waiting
                    .keys()
                    .copied()
                    .min_by_key(|id| (graph.nodes[*id].order, *id))
            });
        let Some(next) = next else { break };
        waiting.remove(next);
        for deps in waiting.values_mut() {
            deps.remove(next);
        }
        out.push(next);
    }
    out
}

/// Prints canonical `.audiograph` text.
pub fn print(graph: &AudioGraph) -> String {
    let mut out = format!("graph {} {{\n", graph.name);
    let mut params: Vec<_> = graph.params.values().collect();
    params.sort_by_key(|p| p.order);
    for p in params {
        let doc = if p.doc.is_empty() {
            String::new()
        } else {
            format!(" {}", string(&p.doc))
        };
        let _ = match &p.kind {
            ParamKind::Value { default, range } => {
                let range = range.map_or(String::new(), |[lo, hi]| {
                    format!(" range({}, {})", number(lo), number(hi))
                });
                writeln!(
                    out,
                    "  param {} = {}{range}{doc};",
                    p.name,
                    number(*default)
                )
            }
            ParamKind::Clip { default } => writeln!(out, "  param {} = {default}{doc};", p.name),
            ParamKind::Trigger => writeln!(out, "  trigger {}{doc};", p.name),
        };
    }
    for id in dependency_order(graph) {
        let node = &graph.nodes[id];
        let ty = node
            .declared
            .map_or(String::new(), |t| format!(": {}", t.name()));
        let _ = writeln!(out, "  let {}{ty} = {};", node.name, call(graph, node));
    }
    for field in Output::ALL {
        if let Some(v) = graph.outputs.get(&field) {
            let _ = writeln!(out, "  output {} = {};", field.name(), value(graph, v));
        }
    }
    out.push_str("}\n");
    out
}
