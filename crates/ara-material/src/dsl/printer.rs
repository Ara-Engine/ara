use crate::{Expr, Material, ParameterValue, TextureDefault, TextureSource, Type, codes};
use ara_diag::Diagnostic;

pub(crate) fn literal(ty: Type, values: &[f64]) -> String {
    if ty == Type::Bool {
        return if values == [1.0] { "true" } else { "false" }.into();
    }
    let values = values
        .iter()
        .map(|&v| ara_json::format_f64(v))
        .collect::<Vec<_>>()
        .join(", ");
    match ty {
        Type::Float => values,
        Type::ColorLinear => format!("linear({values})"),
        Type::ColorSrgb => format!("srgb({values})"),
        _ => format!("{ty}({values})"),
    }
}

pub(crate) fn parameter(ty: Type, value: &ParameterValue) -> String {
    match value {
        ParameterValue::Numeric(values) => literal(ty, values),
        ParameterValue::Bool(value) => value.to_string(),
        ParameterValue::Texture(TextureSource::Asset(reference)) => reference.0.clone(),
        ParameterValue::Texture(TextureSource::Default(default)) => match default {
            TextureDefault::White => "white",
            TextureDefault::Black => "black",
            TextureDefault::FlatNormal => "flat_normal",
        }
        .into(),
    }
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).expect("strings always serialize as JSON")
}

/// Prints parameters in declaration order, lets in dependency/source order, then
/// domain outputs in fixed order. Canonical text never exposes semantic IDs.
pub fn print(m: &Material) -> Result<String, Diagnostic> {
    let finite = m.parameters.values().all(|p| match &p.default {
        ParameterValue::Numeric(v) => v.iter().all(|v| v.is_finite()),
        _ => true,
    }) && m.bindings.values().all(|b| match &b.expr {
        Expr::Literal { values } => values.iter().all(|v| v.is_finite()),
        _ => true,
    });
    if !finite {
        return Err(Diagnostic::error(
            codes::SYNTAX,
            "canonical material text requires finite numbers",
        ));
    }
    let name = |id: &str| {
        m.node_name(id)
            .ok_or_else(|| Diagnostic::error(codes::REFERENCE, format!("unknown binding `{id}`")))
    };
    let mut out = format!(
        "material {} : {} budget {} {{\n",
        m.name, m.domain, m.variant_budget
    );
    for (_, p) in m.ordered_parameters() {
        out.push_str(&format!(
            "  {}param {} : {} = {};\n",
            if p.is_static { "static " } else { "" },
            p.name,
            p.ty,
            parameter(p.ty, &p.default)
        ));
    }
    for (name, f) in &m.functions {
        let args = f
            .inputs
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut costs = vec![format!("alu: {}", f.cost.alu)];
        if f.cost.texture_samples != 0 {
            costs.push(format!("samples: {}", f.cost.texture_samples));
        }
        if f.cost.branches != 0 {
            costs.push(format!("branches: {}", f.cost.branches));
        }
        out.push_str(&format!(
            "  custom {name}({args}) -> {} cost({}) doc {} slang {};\n",
            f.output,
            costs.join(", "),
            quoted(&f.doc),
            quoted(&f.body)
        ));
    }
    for id in crate::check::schedule(m)? {
        let Some(b) = m.bindings.get(&id) else {
            continue;
        };
        let expr = match &b.expr {
            Expr::Literal { values } => literal(b.ty, values),
            Expr::Call { op, args } if op == "copy" && args.len() == 1 => {
                name(&args[0])?.to_owned()
            }
            Expr::Call { op, args } => format!(
                "{}({})",
                op.strip_prefix("custom.").unwrap_or(op),
                args.iter()
                    .map(|id| name(id))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
        };
        out.push_str(&format!("  let {} : {} = {expr};\n", b.name, b.ty));
    }
    for &field in m.domain.output_order() {
        if let Some(id) = m.outputs.get(field) {
            out.push_str(&format!("  output {field} = {};\n", name(id)?));
        }
    }
    // Keep invalid domain fields visible so their diagnostics remain repairable.
    for (field, id) in m
        .outputs
        .iter()
        .filter(|(f, _)| !m.domain.output_order().contains(&f.as_str()))
    {
        out.push_str(&format!("  output {field} = {};\n", name(id)?));
    }
    out.push_str("}\n");
    Ok(out)
}
