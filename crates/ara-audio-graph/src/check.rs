//! Static checks: names, parameter ranges, argument types (with generic items),
//! required and literal-only inputs, cycles and outputs. Only checked graphs compile.

use crate::{
    ast::{AudioGraph, Output, ParamKind, Type, Value},
    codes,
    library::{Op, TypeRule},
};
use ara_diag::Diagnostic;
use std::collections::{BTreeMap, BTreeSet};

/// Largest graph a voice may run.
pub const MAX_NODES: usize = 256;
/// Most gameplay parameters, triggers and clips per graph.
pub const MAX_PARAMS: usize = 64;

/// A graph that passed every check, with evaluation order and resolved output types.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// The graph.
    pub graph: AudioGraph,
    /// Node IDs in evaluation (dependency) order.
    pub order: Vec<String>,
    /// Concrete output types per node ID, in output-port order.
    pub types: BTreeMap<String, Vec<Type>>,
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name != "_"
}

/// Inputs that size or seed state at load time and therefore must be literals.
fn literal_only(op: Op, input: &str) -> bool {
    input == "seed" || (op == Op::Echo && input == "max")
}

impl Checked {
    /// The type of `value`, if it resolves.
    pub(crate) fn value_type(
        graph: &AudioGraph,
        types: &BTreeMap<String, Vec<Type>>,
        value: &Value,
    ) -> Result<Type, String> {
        match value {
            Value::Number { .. } => Ok(Type::Audio),
            Value::Clip { .. } => Ok(Type::Clip),
            Value::Ref { node, port } => {
                if let Some(param) = graph.params.get(node) {
                    return match port {
                        None => Ok(param.ty()),
                        Some(p) => Err(format!("parameter `{}` has no port `{p}`", param.name)),
                    };
                }
                let Some(target) = graph.nodes.get(node) else {
                    return Err(format!("reference to missing node `{node}`"));
                };
                let spec = target.op.spec();
                let index = match port {
                    None => 0,
                    Some(p) => spec.output(p).ok_or_else(|| {
                        let ports: Vec<&str> = spec.outputs.iter().map(|o| o.name).collect();
                        format!(
                            "`{}` ({}) has no output `{p}` (outputs: {})",
                            target.name,
                            spec.name,
                            ports.join(", ")
                        )
                    })?,
                };
                types
                    .get(node)
                    .and_then(|t| t.get(index).copied())
                    .ok_or_else(|| format!("`{}` is used before its inputs check", target.name))
            }
        }
    }
}

/// Checks a graph; all problems are reported together.
pub fn check(graph: &AudioGraph) -> Result<Checked, Vec<Diagnostic>> {
    let error = |message: String| Diagnostic::error(codes::GRAPH, message);
    let mut diags = Vec::new();
    if !valid_name(&graph.name) {
        diags.push(error(format!(
            "graph name `{}` is not an identifier",
            graph.name
        )));
    }
    if graph.nodes.len() > MAX_NODES || graph.params.len() > MAX_PARAMS {
        diags.push(error(format!(
            "graph exceeds {MAX_NODES} nodes or {MAX_PARAMS} parameters"
        )));
    }
    let mut seen = BTreeSet::new();
    let names = graph
        .params
        .iter()
        .map(|(id, p)| (id, &p.name))
        .chain(graph.nodes.iter().map(|(id, n)| (id, &n.name)));
    for (id, name) in names {
        if !valid_name(name) {
            diags.push(error(format!("`{name}` is not an identifier")).record(id));
        }
        if !seen.insert(name.as_str()) {
            diags.push(error(format!("`{name}` is declared twice")).record(id));
        }
        if graph.params.contains_key(id) && graph.nodes.contains_key(id) {
            diags.push(error(format!("ID `{id}` is both a parameter and a node")).record(id));
        }
    }
    for (id, p) in &graph.params {
        match &p.kind {
            ParamKind::Value { default, range } => {
                let bad_range = range.is_some_and(|[lo, hi]| {
                    !lo.is_finite() || !hi.is_finite() || lo >= hi || *default < lo || *default > hi
                });
                if !default.is_finite() || bad_range {
                    diags.push(
                        error(format!(
                            "parameter `{}` needs a finite default inside a lo < hi range",
                            p.name
                        ))
                        .record(id),
                    );
                }
            }
            ParamKind::Clip { default } if !default.starts_with('@') => diags.push(
                error(format!(
                    "clip parameter `{}` must default to an @reference",
                    p.name
                ))
                .record(id),
            ),
            _ => {}
        }
    }
    let order: Vec<String> = crate::dsl::dependency_order(graph)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut types: BTreeMap<String, Vec<Type>> = BTreeMap::new();
    let mut cyclic = false;
    for id in &order {
        let node = &graph.nodes[id];
        let spec = node.op.spec();
        let fail = |message: String| {
            error(format!("`{}` ({}): {message}", node.name, spec.name)).record(id)
        };
        for value in node.args.values() {
            if let Value::Ref { node: target, .. } = value
                && graph.nodes.contains_key(target)
                && !types.contains_key(target)
            {
                cyclic = true;
            }
        }
        let variadic = spec.variadic.map_or(0, |v| {
            (0..v.max)
                .take_while(|i| node.args.contains_key(&format!("in{i}")))
                .count()
        });
        for key in node.args.keys() {
            let known = spec.input(key).is_some()
                || key
                    .strip_prefix("in")
                    .and_then(|n| n.parse::<usize>().ok())
                    .is_some_and(|n| n < variadic);
            if !known {
                diags.push(fail(format!("unknown or non-contiguous input `{key}`")));
            }
        }
        let mut generic: Option<Type> = None;
        if let Some(v) = spec.variadic {
            if variadic < v.min {
                diags.push(fail(format!(
                    "needs at least {} items, has {variadic}",
                    v.min
                )));
            }
            for i in 0..variadic {
                match Checked::value_type(graph, &types, &node.args[&format!("in{i}")]) {
                    Ok(ty) => match (v.ty, generic) {
                        (TypeRule::Is(want), _) if want != ty => diags.push(fail(format!(
                            "item {i} is {}, expected {}",
                            ty.name(),
                            want.name()
                        ))),
                        (TypeRule::Generic, None) => generic = Some(ty),
                        (TypeRule::Generic, Some(bound)) if bound != ty => {
                            diags.push(fail(format!(
                                "item {i} is {}, but earlier items are {}",
                                ty.name(),
                                bound.name()
                            )))
                        }
                        _ => {}
                    },
                    Err(e) => diags.push(fail(e)),
                }
            }
        }
        for input in spec.inputs {
            let Some(value) = node.args.get(input.name) else {
                if input.default.is_none() {
                    diags.push(fail(format!("required input `{}` is missing", input.name)));
                }
                continue;
            };
            if literal_only(node.op, input.name) && !matches!(value, Value::Number { .. }) {
                diags.push(fail(format!("`{}` must be a number literal", input.name)));
                continue;
            }
            let want = match input.ty {
                TypeRule::Is(t) => Some(t),
                TypeRule::Generic => generic,
            };
            match Checked::value_type(graph, &types, value) {
                Ok(ty) if want.is_some_and(|w| w != ty) => diags.push(fail(format!(
                    "input `{}` is {}, expected {}",
                    input.name,
                    ty.name(),
                    want.map_or("?", Type::name)
                ))),
                Ok(_) => {}
                Err(e) => diags.push(fail(e)),
            }
        }
        let outputs: Vec<Type> = spec
            .outputs
            .iter()
            .map(|o| match o.ty {
                TypeRule::Is(t) => t,
                TypeRule::Generic => generic.unwrap_or(Type::Audio),
            })
            .collect();
        if let Some(declared) = node.declared
            && outputs.first() != Some(&declared)
        {
            diags.push(fail(format!(
                "declared {} but produces {}",
                declared.name(),
                outputs.first().map_or("nothing", |t| t.name())
            )));
        }
        types.insert(id.clone(), outputs);
    }
    if cyclic {
        diags.push(error(
            "graph contains a cycle; feedback needs a `delay` node".into(),
        ));
    }
    let has = |o: Output| graph.outputs.contains_key(&o);
    if !(has(Output::Out) || (has(Output::Left) && has(Output::Right)))
        || (has(Output::Out) && (has(Output::Left) || has(Output::Right)))
        || (has(Output::Left) != has(Output::Right))
    {
        diags.push(error(
            "set `output out` (mono) or both `output left` and `output right` (stereo)".into(),
        ));
    }
    for (field, value) in &graph.outputs {
        match Checked::value_type(graph, &types, value) {
            Ok(ty) if ty != field.ty() => diags.push(error(format!(
                "output `{}` needs {}, got {}",
                field.name(),
                field.ty().name(),
                ty.name()
            ))),
            Ok(_) => {}
            Err(e) => diags.push(error(format!("output `{}`: {e}", field.name()))),
        }
    }
    if diags.is_empty() {
        Ok(Checked {
            graph: graph.clone(),
            order,
            types,
        })
    } else {
        Err(diags)
    }
}
