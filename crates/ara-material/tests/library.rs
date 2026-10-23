#![allow(clippy::unwrap_used)] // Test fixtures may panic on violated expectations.

use std::collections::BTreeMap;

use ara_graph::Editor;
use ara_material::*;

fn material(op: &str, types: &[Type], domain: Domain) -> Material {
    let mut m = Material {
        id: "gr_library".into(),
        name: "LibraryCoverage".into(),
        domain,
        variant_budget: 1,
        parameters: BTreeMap::new(),
        bindings: BTreeMap::new(),
        functions: BTreeMap::new(),
        outputs: BTreeMap::new(),
    };
    let args = types
        .iter()
        .enumerate()
        .map(|(i, &ty)| {
            let id = format!("n_arg{i}");
            let mut default = vec![0.3; ty.lanes()];
            if ty.is_texture() {
                default = vec![0.0];
            }
            if matches!(ty, Type::Mat3 | Type::Mat4) {
                let width = if ty == Type::Mat3 { 3 } else { 4 };
                default.fill(0.0);
                for i in 0..width {
                    default[i * width + i] = 1.0;
                }
            }
            m.parameters.insert(
                id.clone(),
                Parameter {
                    name: format!("arg{i}"),
                    ty,
                    default: if ty.is_texture() || ty == Type::Bool {
                        ParameterValue::zero(ty)
                    } else {
                        ParameterValue::Numeric(default)
                    },
                    order: i as u32,
                    is_static: false,
                },
            );
            id
        })
        .collect();
    let ty = library::infer(op, types).unwrap_or_else(|| panic!("signature for {op}/{types:?}"));
    m.bindings.insert(
        "n_result".into(),
        Binding {
            order: m.bindings.len() as u32,
            name: "result".into(),
            ty,
            expr: Expr::Call {
                op: op.into(),
                args,
            },
        },
    );
    let mut current = "n_result".to_owned();
    let mut result = ty;
    let mut call = |op: &str, args: Vec<String>, ty: Type| {
        let id = format!("n_aux{}", m.bindings.len());
        m.bindings.insert(
            id.clone(),
            Binding {
                order: m.bindings.len() as u32,
                name: format!("aux{}", m.bindings.len()),
                ty,
                expr: Expr::Call {
                    op: op.into(),
                    args,
                },
            },
        );
        id
    };
    match ty {
        Type::Mat3 => {
            current = call("transform3", vec![current, "n_arg1".into()], Type::Vec3);
            result = Type::Vec3;
        }
        Type::Mat4 => {
            current = call("transform", vec![current, "n_arg1".into()], Type::Vec4);
            result = Type::Vec4;
        }
        _ => (),
    }
    // Matrix-overload coverage appends a vector with an ID separate from call inputs.
    if matches!(ty, Type::Mat3 | Type::Mat4) {
        let vector_ty = if ty == Type::Mat3 {
            Type::Vec3
        } else {
            Type::Vec4
        };
        // n_arg1 may already be a matrix input; use a dedicated vector instead.
        if let Expr::Call { args, .. } = &mut m.bindings.get_mut(&current).unwrap().expr {
            args[1] = "n_vector".into();
        }
        m.parameters.insert(
            "n_vector".into(),
            Parameter {
                name: "vector".into(),
                ty: vector_ty,
                default: ParameterValue::Numeric(vec![0.5; vector_ty.lanes()]),
                order: m.parameters.len() as u32,
                is_static: false,
            },
        );
    }
    if result == Type::Vec4 {
        let id = "n_rgb".to_owned();
        m.bindings.insert(
            id.clone(),
            Binding {
                order: m.bindings.len() as u32,
                name: "rgbValue".into(),
                ty: Type::Vec3,
                expr: Expr::Call {
                    op: "rgb".into(),
                    args: vec![current],
                },
            },
        );
        current = id;
        result = Type::Vec3;
    }
    if result == Type::Vec2 {
        let id = "n_dot".to_owned();
        m.bindings.insert(
            id.clone(),
            Binding {
                order: m.bindings.len() as u32,
                name: "squaredLength".into(),
                ty: Type::Float,
                expr: Expr::Call {
                    op: "dot".into(),
                    args: vec![current.clone(), current],
                },
            },
        );
        current = id;
        result = Type::Float;
    }
    if result == Type::Vec3 || result == Type::ColorSrgb {
        let op = if result == Type::Vec3 {
            "color"
        } else {
            "linear"
        };
        let id = "n_color".to_owned();
        m.bindings.insert(
            id.clone(),
            Binding {
                order: m.bindings.len() as u32,
                name: "linearColor".into(),
                ty: Type::ColorLinear,
                expr: Expr::Call {
                    op: op.into(),
                    args: vec![current],
                },
            },
        );
        current = id;
        result = Type::ColorLinear;
    }
    let output = if result == Type::Float {
        if domain == Domain::Surface {
            "roughness"
        } else {
            "opacity"
        }
    } else if matches!(domain, Domain::Surface | Domain::Decal) {
        "baseColor"
    } else {
        "color"
    };
    m.outputs.insert(output.into(), current);
    m.normalize_order().unwrap();
    m
}

fn inputs(op: &str) -> Vec<Type> {
    use Type::*;
    match op {
        "copy" => vec![Vec3],
        "float" => vec![Bool],
        "add" | "sub" | "mul" | "div" | "min" | "max" | "pow" | "step" => vec![Vec3, Vec3],
        "clamp" | "smoothstep" => vec![Vec3, Vec3, Vec3],
        "lerp" => vec![ColorLinear, ColorLinear, Float],
        "abs" | "sin" | "cos" | "floor" | "fract" | "saturate" | "sqrt" | "normalize"
        | "length" => vec![Vec3],
        "dot" | "cross" | "normalBlend" => vec![Vec3, Vec3],
        "uv" | "worldPosition" | "worldNormal" | "viewDirection" | "time" => vec![],
        "sampleColor" => vec![TextureColor, Vec2],
        "sampleNormal" => vec![TextureNormal, Vec2],
        "sampleData" => vec![TextureData, Vec2],
        "linear" => vec![ColorSrgb],
        "srgb" | "color" => vec![Vec3],
        "rgb" | "alpha" => vec![Vec4],
        "luminance" => vec![ColorLinear],
        "normalStrength" => vec![Vec3, Float],
        "noise" | "sceneColor" => vec![Vec2],
        "triplanar" => vec![TextureColor, Vec3, Vec3, Float],
        "parallax" => vec![TextureData, Vec2, Vec3, Float],
        "fresnel" => vec![Vec3, Vec3, Float],
        "rotateUV" => vec![Vec2, Float],
        "transform" => vec![Mat4, Vec4],
        "transform3" => vec![Mat3, Vec3],
        "roundedPanel" => vec![Vec2, Vec2, Float],
        _ => panic!("missing library test for {op}"),
    }
}

#[test]
fn every_starter_node_roundtrips_and_compiles_both_targets() {
    let compiler = ShaderCompiler::discover();
    for builtin in library::BUILTINS {
        let domain = match builtin.name {
            "roundedPanel" => Domain::Ui,
            "sceneColor" => Domain::PostProcess,
            _ => Domain::Surface,
        };
        let m = material(builtin.name, &inputs(builtin.name), domain);
        check(&m).unwrap();
        let text = dsl::print(&m).unwrap();
        assert_eq!(m, dsl::reconcile(&text, &m).unwrap().material);
        assert_eq!(
            m,
            Material::from_graph(&m.to_graph(Editor::default()).unwrap()).unwrap()
        );
        let generated = generate(&m, &Instance::default()).unwrap();
        for target in [ShaderTarget::Spirv, ShaderTarget::Dxil] {
            compiler
                .compile(&generated, target)
                .unwrap_or_else(|e| panic!("{}/{target:?}: {e}", builtin.name));
        }
    }
}

#[test]
fn matrix_overloads_packing_and_all_domain_contracts() {
    let compiler = ShaderCompiler::discover();
    for ty in [Type::Mat3, Type::Mat4] {
        let m = material("mul", &[ty, ty], Domain::Surface);
        let packed = Instance::default().pack(&m, |_, _| Ok(0)).unwrap();
        assert_eq!(packed.slots[0].rows, if ty == Type::Mat3 { 3 } else { 4 });
        if ty == Type::Mat3 {
            assert_eq!(packed.rows[0], [1.0, 0.0, 0.0, 0.0]);
            assert_eq!(packed.rows[1], [0.0, 1.0, 0.0, 0.0]);
        }
        for target in [ShaderTarget::Spirv, ShaderTarget::Dxil] {
            compiler
                .compile(&generate(&m, &Instance::default()).unwrap(), target)
                .unwrap();
        }
    }
    for domain in Domain::ALL {
        let m = material("color", &[Type::Vec3], *domain);
        check(&m).unwrap();
        for target in [ShaderTarget::Spirv, ShaderTarget::Dxil] {
            compiler
                .compile(&generate(&m, &Instance::default()).unwrap(), target)
                .unwrap();
        }
    }
}
