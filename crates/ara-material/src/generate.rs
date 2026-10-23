use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};

use crate::{
    Cost, Domain, Expr, Instance, Material, ParameterSlot, ParameterValue, Type, VariantKey, check,
    fold_constants, library, parameter_layout,
};

/// Engine-owned bindings, bounded helpers and lighting template, ABI version 1.
pub const ENGINE_TEMPLATE: &str = include_str!("../shaders/engine.slang");

/// Inclusive 1-based generated source span for one semantic material node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineMapping {
    /// Persistent material node ID.
    pub node: String,
    /// First generated line.
    pub start_line: u32,
    /// Last generated line.
    pub end_line: u32,
}

/// Offline compiler input and editor-generated-code view, never runtime compilation.
#[derive(Debug, Clone)]
pub struct Generated {
    /// Complete Slang module, including the engine template and validation entry.
    pub source: String,
    /// Bidirectional node to generated source locations (custom bodies included).
    pub mappings: Vec<LineMapping>,
    /// Reachable, folded shader work estimate; custom costs are author declarations.
    pub cost: Cost,
    /// Runtime material buffer packing contract.
    pub parameters: Vec<ParameterSlot>,
    /// Static specialization/content identity; excludes instance runtime values/layout.
    pub variant: VariantKey,
}

impl Generated {
    /// Resolves compiler line locations back to all contributing semantic nodes.
    pub fn nodes_at_line(&self, line: u32) -> impl Iterator<Item = &str> {
        self.mappings
            .iter()
            .filter(move |m| m.start_line <= line && line <= m.end_line)
            .map(|m| m.node.as_str())
    }
}

fn literal(ty: Type, values: &[f64]) -> String {
    if ty == Type::Bool {
        return if values == [1.0] { "true" } else { "false" }.into();
    }
    let lanes = values
        .iter()
        .map(|&n| {
            let n = ara_json::format_f64(f64::from(n as f32));
            if n.contains(['.', 'e', 'E']) {
                n
            } else {
                format!("{n}.0")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({lanes})", ty.slang())
}

fn parameter(slot: &ParameterSlot) -> String {
    let offset = slot.offset;
    match slot.ty {
        Type::Bool => format!("(ara_load({offset}u).x != 0.0)"),
        Type::Mat3 => format!(
            "float3x3(ara_load({offset}u).xyz, ara_load({}u).xyz, ara_load({}u).xyz)",
            offset + 1,
            offset + 2
        ),
        Type::Mat4 => format!(
            "float4x4(ara_load({offset}u), ara_load({}u), ara_load({}u), ara_load({}u))",
            offset + 1,
            offset + 2,
            offset + 3
        ),
        ty if ty.is_texture() => format!("uint(ara_load({offset}u).x)"),
        ty => format!(
            "ara_load({offset}u).{}",
            match ty.lanes() {
                1 => "x",
                2 => "xy",
                3 => "xyz",
                _ => "xyzw",
            }
        ),
    }
}

fn expression(op: &str, args: &[String], types: &[Type]) -> String {
    let a = args.join(", ");
    match op {
        "copy" => args[0].clone(),
        "add" => format!("({} + {})", args[0], args[1]),
        "sub" => format!("({} - {})", args[0], args[1]),
        "mul" if matches!(types[0], Type::Mat3 | Type::Mat4) && types[0] == types[1] => {
            format!("mul({a})")
        }
        "mul" => format!("({} * {})", args[0], args[1]),
        "div" => format!("({} / {})", args[0], args[1]),
        "uv" => "input.uv".into(),
        "worldPosition" => "input.worldPosition".into(),
        "worldNormal" => "input.worldNormal".into(),
        "viewDirection" => "input.viewDirection".into(),
        "time" => "draw.phase".into(),
        "sampleColor" => format!("ara_sample_color({a})"),
        "sampleNormal" => format!("ara_sample_normal({a})"),
        "sampleData" => format!("ara_sample_data({a})"),
        "linear" => format!("ara_linear({a})"),
        "srgb" | "color" => args[0].clone(),
        "rgb" => format!("{}.rgb", args[0]),
        "alpha" => format!("{}.a", args[0]),
        "luminance" => format!("dot({}, float3(0.2126, 0.7152, 0.0722))", args[0]),
        "normalBlend" => format!("ara_normal_blend({a})"),
        "normalStrength" => format!("ara_normal_strength({a})"),
        "noise" => format!("ara_noise({a})"),
        "triplanar" => format!("ara_triplanar({a})"),
        "parallax" => format!("ara_parallax({a})"),
        "fresnel" => format!("ara_fresnel({a})"),
        "rotateUV" => format!("ara_rotate_uv({a})"),
        "transform" | "transform3" => format!("mul({a})"),
        "roundedPanel" => format!("ara_rounded_panel({a})"),
        "normalize" => format!("ara_normalize({a})"),
        "sceneColor" => format!("ara_sample_data(draw.textureIndex, {a}).rgb"),
        "fract" => format!("frac({a})"),
        op if op.starts_with("custom.") => format!("ara_custom_{}({a})", &op[7..]),
        _ => format!("{op}({a})"),
    }
}

/// Generates checked, constant-folded surface evaluation plus engine lighting.
/// Root constants and bindless declarations exactly follow S1; compilation is offline.
pub fn generate(m: &Material, instance: &Instance) -> Result<Generated, Vec<Diagnostic>> {
    check(m)?;
    instance.validate(m)?;
    let variant = instance.variant_key(m)?;
    let parameters = parameter_layout(m);
    let folded = fold_constants(m)?;
    let checked = check(&folded)?;
    let slots: BTreeMap<_, _> = parameters.iter().map(|p| (p.node.as_str(), p)).collect();
    let mut source = ENGINE_TEMPLATE.to_owned();
    let mut mappings = Vec::new();
    let mut cost = Cost::default();
    for (name, f) in &m.functions {
        let users: Vec<_> = m
            .bindings
            .iter()
            .filter(|(id, b)| {
                checked.reachable.contains(*id)
                    && matches!(&b.expr,Expr::Call{op,..} if op==&format!("custom.{name}"))
            })
            .map(|(id, _)| id)
            .collect();
        if users.is_empty() {
            continue;
        }
        let inputs = f
            .inputs
            .iter()
            .map(|(n, t)| format!("{} {n}", t.slang()))
            .collect::<Vec<_>>()
            .join(", ");
        let start_line = source.lines().count() as u32 + 1;
        source.push_str(&format!(
            "\n{} ara_custom_{name}({inputs}) {{\n{}\n}}\n",
            f.output.slang(),
            f.body
        ));
        let end_line = source.lines().count() as u32;
        for id in users {
            mappings.push(LineMapping {
                node: id.clone(),
                start_line,
                end_line,
            });
        }
    }
    source.push_str("\nAraSurface ara_evaluate(AraMaterialInput input) {\n    AraSurface result;\n    result.baseColor = float3(1);\n    result.metallic = 0;\n    result.roughness = 0.5;\n    result.normal = float3(0, 0, 1);\n    result.emissive = float3(0);\n    result.opacityMask = 1;\n    result.clearCoat = 0;\n    result.clearCoatRoughness = 0.1;\n    result.transmission = 0;\n    result.color = float3(1);\n    result.opacity = 1;\n    result.clip = 1;\n");
    for id in checked
        .order
        .iter()
        .filter(|id| checked.reachable.contains(*id))
    {
        let ty = m
            .node_type(id)
            .expect("checked schedule refers only to existing typed nodes");
        let expression = if let Some(p) = m.parameters.get(id) {
            if !p.is_static {
                parameter(slots[id.as_str()])
            } else {
                match instance.overrides.get(id).unwrap_or(&p.default) {
                    ParameterValue::Bool(v) => v.to_string(),
                    _ => unreachable!("type checking accepts only boolean static parameters"),
                }
            }
        } else {
            match &folded.bindings[id].expr {
                Expr::Literal { values } => literal(ty, values),
                Expr::Call { op, args } => {
                    let c = library::get(op)
                        .map(|b| b.cost)
                        .or_else(|| {
                            op.strip_prefix("custom.")
                                .and_then(|n| m.functions.get(n).map(|f| f.cost))
                        })
                        .unwrap_or_default();
                    cost.texture_samples = cost.texture_samples.saturating_add(c.texture_samples);
                    cost.alu = cost.alu.saturating_add(c.alu);
                    cost.branches = cost.branches.saturating_add(c.branches);
                    let types = args
                        .iter()
                        .map(|id| {
                            m.node_type(id)
                                .expect("type checking resolved every argument")
                        })
                        .collect::<Vec<_>>();
                    expression(op, args, &types)
                }
            }
        };
        let line = source.lines().count() as u32 + 1;
        source.push_str(&format!("    {} {id} = {expression};\n", ty.slang()));
        mappings.push(LineMapping {
            node: id.clone(),
            start_line: line,
            end_line: line,
        });
    }
    for (field, id) in &m.outputs {
        let line = source.lines().count() as u32 + 1;
        source.push_str(&format!("    result.{field} = {id};\n"));
        mappings.push(LineMapping {
            node: "n_output".into(),
            start_line: line,
            end_line: line,
        });
    }
    source.push_str("    return result;\n}\n");
    source.push_str("\n// Offline verification entry. Renderer passes call ara_evaluate directly.\nstruct AraValidationInput {\n    float4 position : SV_Position;\n    float2 uv : TEXCOORD0;\n    float3 worldPosition : TEXCOORD1;\n    float3 worldNormal : TEXCOORD2;\n    float3 worldTangent : TEXCOORD3;\n    float3 worldBitangent : TEXCOORD4;\n    float3 viewDirection : TEXCOORD5;\n};\n[shader(\"fragment\")]\nfloat4 fragmentMain(AraValidationInput v) : SV_Target {\n    AraMaterialInput input;\n    input.uv = v.uv; input.worldPosition = v.worldPosition;\n    input.worldNormal = v.worldNormal; input.worldTangent = v.worldTangent;\n    input.worldBitangent = v.worldBitangent; input.viewDirection = v.viewDirection;\n    AraSurface s = ara_evaluate(input);\n");
    source.push_str(match m.domain {
        Domain::Surface=>"    clip(s.opacityMask - 0.5);\n    return float4(ara_engine_light(s, input, float3(0, 1, 0), float3(1), float3(0.03)), 1);\n",
        Domain::Decal=>"    return float4(s.baseColor + s.emissive, saturate(s.opacity));\n",
        Domain::Unlit=>"    clip(s.opacityMask - 0.5);\n    return float4(s.color, saturate(s.opacity));\n",
        Domain::Ui=>"    clip(s.clip - 0.5);\n    float alpha = saturate(s.opacity);\n    return float4(s.color * alpha, alpha);\n",
        Domain::PostProcess=>"    return float4(s.color, saturate(s.opacity));\n",
    });
    source.push_str("}\n");
    Ok(Generated {
        source,
        mappings,
        cost,
        parameters,
        variant,
    })
}
