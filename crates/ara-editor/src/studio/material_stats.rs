//! The Material page's stats strip, as Unreal's Stats panel reads: whether the material
//! checks, its first error, and the static cost of what reaches its outputs (arithmetic
//! and texture samples, from the library's estimates), its static variants against the
//! budget, and its node count. Estimates, not GPU measurements.
use ara_kit::icons;
use ara_material::{Cost, Expr, Material};
use stillkit::el::{Align, El, el, icon, row, text};

use super::Studio;

/// The summed estimate of the nodes that reach the outputs.
fn cost(material: &Material, reachable: &std::collections::BTreeSet<String>) -> Cost {
    reachable
        .iter()
        .filter_map(|id| match &material.bindings.get(id)?.expr {
            Expr::Call { op, .. } => match op.strip_prefix("custom.") {
                Some(name) => material.functions.get(name).map(|f| f.cost),
                None => ara_material::library::get(op).map(|b| b.cost),
            },
            Expr::Literal { .. } => None,
        })
        .fold(Cost::default(), |a, b| Cost {
            texture_samples: a.texture_samples + b.texture_samples,
            alu: a.alu + b.alu,
            branches: a.branches + b.branches,
        })
}

/// What the strip shows, worked out once per loaded revision ([`summarize`]).
pub(super) struct Summary {
    /// The estimate and static variants with their budget, or the problems and the first.
    checked: Result<(Cost, u32, u32), (usize, String)>,
    /// Nodes drawn (inline defaults not counted).
    nodes: usize,
}

/// The stats of material `graph`.
pub(super) fn summarize(graph: &ara_graph::Graph) -> Summary {
    let first = |diags: Vec<ara_diag::Diagnostic>| {
        let first = diags.first().map(|d| d.message.clone()).unwrap_or_default();
        (diags.len(), first)
    };
    let checked = Material::from_graph(graph)
        .map_err(first)
        .and_then(|material| {
            let checked = ara_material::check(&material).map_err(first)?;
            let statics = material.parameters.values().filter(|p| p.is_static).count() as u32;
            Ok((
                cost(&material, &checked.reachable),
                1u32.checked_shl(statics).unwrap_or(u32::MAX),
                material.variant_budget,
            ))
        });
    Summary {
        checked,
        nodes: graph
            .nodes
            .keys()
            .filter(|id| !crate::graph::is_default(graph, id))
            .count(),
    }
}

/// The strip under the canvas.
pub(super) fn strip(s: &Studio, summary: &Summary) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let item = |glyph, label: String| {
        row()
            .gap(5.0)
            .align(Align::Center)
            .child(icon(glyph).size(12.0).color(c.muted_foreground.0))
            .child(text(label).text_size(11.5).color(c.muted_foreground.0))
    };
    let mut out = row()
        .key("material/stats")
        .h(26.0)
        .pad_x(10.0)
        .gap(14.0)
        .align(Align::Center)
        .fill(super::style::tones(l).strip);
    out = match &summary.checked {
        Ok((cost, variants, budget)) => out
            .child(
                row()
                    .gap(6.0)
                    .align(Align::Center)
                    .child(el().size(7.0).radius(3.5).fill(c.success_foreground.0))
                    .child(text("Compiles").text_size(11.5).color(c.muted_foreground.0)),
            )
            .child(item(icons::CPU, format!("~{} ALU", cost.alu)))
            .child(item(
                icons::IMAGE,
                format!(
                    "{} texture sample{}",
                    cost.texture_samples,
                    if cost.texture_samples == 1 { "" } else { "s" }
                ),
            ))
            .child(item(
                icons::LAYERS,
                format!("{variants} of {budget} variants"),
            ))
            .child(el().grow(1.0)),
        Err((count, first)) => out.child(problem(s, *count, first)),
    };
    out.child(item(icons::WORKFLOW, format!("{} nodes", summary.nodes)))
}

/// A failed check: how many problems, and the first.
fn problem(s: &Studio, count: usize, first: &str) -> El<Studio> {
    let c = &s.look.c;
    row()
        .gap(6.0)
        .align(Align::Center)
        .w(0.0)
        .grow(1.0)
        .child(
            icon(icons::CIRCLE_ALERT)
                .size(12.0)
                .color(c.destructive_foreground.0),
        )
        .child(
            text(format!(
                "{count} problem{}{}",
                if count == 1 { "" } else { "s" },
                if first.is_empty() {
                    String::new()
                } else {
                    format!(" · {first}")
                }
            ))
            .text_size(11.5)
            .color(c.destructive_foreground.0)
            .wrap(1)
            .w(0.0)
            .grow(1.0),
        )
}
