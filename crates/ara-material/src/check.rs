use std::collections::{BTreeMap, BTreeSet};

use ara_diag::{Diagnostic, FixAction};
use ara_schema::grammar::split_id;
use serde_json::json;

use crate::{Expr, Material, ParameterValue, TextureDefault, TextureSource, Type, codes, library};

/// A checked AST with deterministic scheduling and output-reachable node IDs.
#[derive(Debug)]
pub struct CheckedMaterial<'a> {
    /// Original AST, borrowed without side effects.
    pub material: &'a Material,
    /// Dependency order, including disconnected authoring nodes.
    pub order: Vec<String>,
    /// Nodes that affect domain outputs and therefore compilation cost.
    pub reachable: BTreeSet<String>,
}

pub(crate) fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(crate) fn value_name(name: &str) -> bool {
    identifier(name)
        && !matches!(
            name,
            "true"
                | "false"
                | "white"
                | "black"
                | "flat_normal"
                | "material"
                | "param"
                | "let"
                | "static"
                | "output"
                | "custom"
                | "budget"
                | "cost"
                | "doc"
                | "slang"
        )
}

pub(crate) fn valid_values(ty: Type, values: &[f64]) -> bool {
    !ty.is_texture()
        && values.len() == ty.lanes()
        && values
            .iter()
            .all(|v| v.is_finite() && (*v as f32).is_finite())
        && (ty != Type::Bool || matches!(values, [0.0] | [1.0]))
}

pub(crate) fn valid_value(ty: Type, value: &ParameterValue) -> bool {
    match value {
        ParameterValue::Numeric(v) => ty != Type::Bool && valid_values(ty, v),
        ParameterValue::Bool(_) => ty == Type::Bool,
        ParameterValue::Texture(TextureSource::Asset(reference)) => {
            ty.is_texture() && ara_schema::grammar::parse_ref(&reference.0).is_ok()
        }
        ParameterValue::Texture(TextureSource::Default(default)) => match default {
            TextureDefault::FlatNormal => ty == Type::TextureNormal,
            TextureDefault::White | TextureDefault::Black => {
                matches!(ty, Type::TextureColor | Type::TextureData)
            }
        },
    }
}

fn literal_fix(m: &Material, id: &str, message: impl Into<String>) -> Diagnostic {
    let ty = m.node_type(id).unwrap_or(Type::Float);
    Diagnostic::error(codes::TYPE, message).record(id).with_fix(
        "Replace with a valid typed zero constant",
        FixAction::Command {
            command: "material.node.literal".into(),
            params: json!({"node":id,"values":vec![0.0;ty.lanes()]}),
        },
    )
}

pub(crate) fn schedule(m: &Material) -> Result<Vec<String>, Diagnostic> {
    let mut degree: BTreeMap<&str, usize> = m
        .parameters
        .keys()
        .chain(m.bindings.keys())
        .map(|id| (id.as_str(), 0))
        .collect();
    let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, b) in &m.bindings {
        if let Expr::Call { args, .. } = &b.expr {
            for arg in args {
                if !degree.contains_key(arg.as_str()) {
                    return Err(Diagnostic::error(
                        codes::REFERENCE,
                        format!("missing binding `{arg}`"),
                    )
                    .record(id));
                }
                if let Some(d) = degree.get_mut(id.as_str()) {
                    *d += 1;
                }
                successors.entry(arg).or_default().push(id);
            }
        }
    }
    let priority = |id: &str| {
        m.bindings
            .get(id)
            .map_or((0, 0, id.to_owned()), |b| (1, b.order, id.to_owned()))
    };
    let mut ready: BTreeSet<_> = degree
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(&id, _)| priority(id))
        .collect();
    let mut result = Vec::new();
    while let Some((_, _, id)) = ready.pop_first() {
        result.push(id.clone());
        for next in successors.get(id.as_str()).into_iter().flatten() {
            if let Some(n) = degree.get_mut(next) {
                *n -= 1;
                if *n == 0 {
                    ready.insert(priority(next));
                }
            }
        }
    }
    if result.len() != degree.len() {
        return Err(Diagnostic::error(
            ara_graph::codes::CYCLE,
            "material expressions must form a DAG",
        ));
    }
    Ok(result)
}

/// Checks all expressions, resources, custom signatures, domains and variants.
/// Type diagnostics include commands that can be applied in an undoable batch.
pub fn check(m: &Material) -> Result<CheckedMaterial<'_>, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    if split_id(&m.id).is_none_or(|(p, _)| p != "gr") || !identifier(&m.name) {
        diags.push(Diagnostic::error(
            codes::SYNTAX,
            "material needs a gr_ asset ID and an identifier name",
        ));
    }
    let mut names = BTreeSet::new();
    for id in m.parameters.keys().chain(m.bindings.keys()) {
        if id == "n_output" || split_id(id).is_none_or(|(p, _)| p != "n") {
            diags.push(
                Diagnostic::error(codes::SYNTAX, "node needs an n_ ID; n_output is reserved")
                    .record(id),
            );
        }
        let name = m.node_name(id).unwrap_or("");
        if !value_name(name) || !names.insert(name) {
            diags.push(
                Diagnostic::error(
                    codes::SYNTAX,
                    "binding names must be unique, nonreserved identifiers",
                )
                .record(id),
            );
        }
    }
    for (id, p) in &m.parameters {
        if m.bindings.contains_key(id) {
            diags.push(
                Diagnostic::error(codes::SYNTAX, "parameter and expression IDs overlap").record(id),
            );
        }
        if !valid_value(p.ty, &p.default) {
            diags.push(
                Diagnostic::error(
                    codes::TYPE,
                    "parameter needs finite typed values, a boolean, or a semantic texture asset/default",
                )
                .record(id)
                .with_fix(
                    "Reset to a valid typed default",
                    FixAction::Command {
                        command: "material.parameter.reset".into(),
                        params: json!({"node":id}),
                    },
                ),
            );
        }
        if p.is_static && p.ty != Type::Bool {
            diags.push(
                Diagnostic::error(
                    codes::OVERRIDE,
                    "static switches must have type bool; their variant set is false/true",
                )
                .record(id),
            );
        }
    }
    for (name, f) in &m.functions {
        let mut args = BTreeSet::new();
        if !value_name(name)
            || library::get(name).is_some()
            || names.contains(name.as_str())
            || f.doc.trim().is_empty()
            || f.body.trim().is_empty()
            || f.inputs
                .iter()
                .any(|(n, _)| !identifier(n) || !args.insert(n))
        {
            diags.push(Diagnostic::error(
                codes::REFERENCE,
                format!("custom `{name}` needs a unique, nonreserved function name, identifier inputs, docs and a nonempty body"),
            ));
        }
    }
    for (id, b) in &m.bindings {
        match &b.expr {
            Expr::Literal { values } if !valid_values(b.ty, values) => diags.push(literal_fix(
                m,
                id,
                "constant shape/range does not match its type",
            )),
            Expr::Call { op, args } => {
                let types: Option<Vec<Type>> = args.iter().map(|id| m.node_type(id)).collect();
                let Some(types) = types else {
                    diags.push(
                        Diagnostic::error(codes::REFERENCE, "call refers to an absent expression")
                            .record(id),
                    );
                    continue;
                };
                let inferred = if let Some(name) = op.strip_prefix("custom.") {
                    match m.functions.get(name) {
                        Some(f) if f.inputs.iter().map(|(_, t)| *t).eq(types.iter().copied()) => {
                            Some(f.output)
                        }
                        Some(_) => None,
                        None => {
                            diags.push(
                                Diagnostic::error(
                                    codes::REFERENCE,
                                    format!("unknown custom function `{name}`"),
                                )
                                .record(id),
                            );
                            continue;
                        }
                    }
                } else {
                    if library::get(op).is_none() {
                        diags.push(
                            Diagnostic::error(
                                codes::REFERENCE,
                                format!("unknown operation `{op}`"),
                            )
                            .record(id),
                        );
                        continue;
                    }
                    if !library::available(op, m.domain) {
                        diags.push(
                            Diagnostic::error(
                                codes::DOMAIN,
                                format!("`{op}` is unavailable in {}", m.domain),
                            )
                            .record(id),
                        );
                    }
                    library::infer(op, &types)
                };
                match inferred {
                    None => diags.push(literal_fix(
                        m,
                        id,
                        format!(
                            "`{op}` does not accept ({})",
                            types
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )),
                    Some(ty) if ty != b.ty => diags.push(
                        Diagnostic::error(
                            codes::TYPE,
                            format!("declared {}, but `{op}` returns {ty}", b.ty),
                        )
                        .record(id)
                        .with_fix(
                            format!("Change annotation to {ty}"),
                            FixAction::Command {
                                command: "material.node.type".into(),
                                params: json!({"node":id,"ty":ty.name()}),
                            },
                        ),
                    ),
                    _ => (),
                }
            }
            _ => (),
        }
    }
    let outputs = m.domain.outputs();
    if m.outputs.is_empty() {
        diags.push(Diagnostic::error(
            codes::DOMAIN,
            "material must expose at least one domain output",
        ));
    }
    for (field, id) in &m.outputs {
        match (outputs.get(field.as_str()), m.node_type(id)) {
            (None, _) => diags.push(Diagnostic::error(
                codes::DOMAIN,
                format!("output `{field}` is not part of {}", m.domain),
            )),
            (_, None) => diags.push(Diagnostic::error(
                codes::REFERENCE,
                format!("output `{field}` refers to missing `{id}`"),
            )),
            (Some(expected), Some(actual)) if *expected != actual => diags.push(
                Diagnostic::error(
                    codes::TYPE,
                    format!("output `{field}` needs {expected}, found {actual}"),
                )
                .record(id)
                .with_fix(
                    "Reset this output to a correctly typed default",
                    FixAction::Command {
                        command: "material.output.default".into(),
                        params: json!({"field":field}),
                    },
                ),
            ),
            _ => (),
        }
    }
    let combinations = m
        .parameters
        .values()
        .filter(|p| p.is_static)
        .fold(1u64, |n, _| n.saturating_mul(2));
    if combinations > u64::from(m.variant_budget) {
        diags.push(
            Diagnostic::error(
                codes::VARIANT_BUDGET,
                format!(
                    "{combinations} declared variants exceed budget {}",
                    m.variant_budget
                ),
            )
            .with_fix(
                "Raise the budget explicitly after reviewing compile cost",
                FixAction::Command {
                    command: "material.budget.set".into(),
                    params: json!({"budget":combinations.min(u64::from(u32::MAX))}),
                },
            ),
        );
    }
    let order = match schedule(m) {
        Ok(o) => o,
        Err(d) => {
            diags.push(d);
            Vec::new()
        }
    };
    if !diags.is_empty() {
        return Err(diags);
    }
    let mut reachable = BTreeSet::new();
    let mut pending: Vec<_> = m.outputs.values().cloned().collect();
    while let Some(id) = pending.pop() {
        if reachable.insert(id.clone())
            && let Some(b) = m.bindings.get(&id)
            && let Expr::Call { args, .. } = &b.expr
        {
            pending.extend(args.iter().cloned());
        }
    }
    Ok(CheckedMaterial {
        material: m,
        order,
        reachable,
    })
}

/// Folds finite constant expressions without changing node IDs or the source AST.
/// Runtime parameters, textures, context inputs and opaque code remain dynamic.
pub fn fold_constants(m: &Material) -> Result<Material, Vec<Diagnostic>> {
    let checked = check(m)?;
    let mut folded = m.clone();
    let mut constants: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for id in &checked.order {
        let Some(b) = folded.bindings.get(id) else {
            continue;
        };
        let values = match &b.expr {
            Expr::Literal { values } => Some(values.iter().map(|&v| f64::from(v as f32)).collect()),
            Expr::Call { op, args } => {
                let inputs: Option<Vec<_>> = args
                    .iter()
                    .map(|id| constants.get(id).map(Vec::as_slice))
                    .collect();
                inputs.and_then(|v| fold(op, &v, b.ty))
            }
        };
        if let Some(values) = values.filter(|v| valid_values(b.ty, v)) {
            constants.insert(id.clone(), values.clone());
            if let Some(b) = folded.bindings.get_mut(id) {
                b.expr = Expr::Literal { values };
            }
        }
    }
    Ok(folded)
}

fn fold(op: &str, a: &[&[f64]], ty: Type) -> Option<Vec<f64>> {
    let first = *a.first()?;
    let binary = |f: fn(f32, f32) -> f32| -> Option<Vec<f64>> {
        let b = *a.get(1)?;
        Some(
            first
                .iter()
                .enumerate()
                .map(|(i, &x)| f64::from(f(x as f32, b[if b.len() == 1 { 0 } else { i }] as f32)))
                .collect(),
        )
    };
    let unary = |f: fn(f32) -> f32| Some(first.iter().map(|&x| f64::from(f(x as f32))).collect());
    match op {
        "add" => binary(|x, y| x + y),
        "sub" => binary(|x, y| x - y),
        "mul" if !matches!(ty, Type::Mat3 | Type::Mat4) => binary(|x, y| x * y),
        "div" => binary(|x, y| x / y),
        "min" => binary(f32::min),
        "max" => binary(f32::max),
        "pow" => binary(f32::powf),
        "abs" => unary(f32::abs),
        "sin" => unary(f32::sin),
        "cos" => unary(f32::cos),
        "sqrt" => unary(f32::sqrt),
        "floor" => unary(f32::floor),
        "fract" => unary(|x| x - x.floor()),
        "saturate" => unary(|x| x.clamp(0.0, 1.0)),
        "linear" => unary(|x| {
            if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        }),
        "copy" | "srgb" | "color" => Some(first.to_vec()),
        "rgb" => Some(first[..3].to_vec()),
        "alpha" => Some(vec![first[3]]),
        "dot" => Some(vec![f64::from(
            first
                .iter()
                .zip(*a.get(1)?)
                .map(|(&x, &y)| x as f32 * y as f32)
                .sum::<f32>(),
        )]),
        "length" => Some(vec![f64::from(
            first
                .iter()
                .map(|&x| (x as f32) * (x as f32))
                .sum::<f32>()
                .sqrt(),
        )]),
        "normalize" => {
            let squared = first
                .iter()
                .map(|&x| (x as f32) * (x as f32))
                .sum::<f32>()
                .max(1e-20);
            let reciprocal = 1.0 / squared.sqrt();
            Some(
                first
                    .iter()
                    .map(|&x| f64::from(x as f32 * reciprocal))
                    .collect(),
            )
        }
        "lerp" => {
            let b = *a.get(1)?;
            let t = a.get(2)?[0] as f32;
            Some(
                first
                    .iter()
                    .zip(b)
                    .map(|(&x, &y)| f64::from(x as f32 + (y as f32 - x as f32) * t))
                    .collect(),
            )
        }
        _ => None,
    }
}
