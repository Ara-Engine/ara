//! In-node previews (Shader Graph): an operation node's value as its own Unlit material.
//! The derived material keeps the source's parameters and bindings, adds the few typed
//! conversions that turn the node's value into colour, and outputs only that. Values
//! that depend on world-space inputs show on the studio sphere; the rest show on a
//! head-on UV swatch. Raw values are shown (no lighting or tone curve).
use super::material_preview::{Shape, Shot};
use ara_material::{Binding, Domain, Expr, Material, Type};
use std::collections::BTreeMap;

/// Most previews derived per graph: each is its own shader, and they share the bounded
/// thumbnail image pool with the Assets grid.
pub(crate) const MAX: usize = 16;

/// A material with UV, noise, colour and world-space operations: the capture route's
/// in-node preview showcase.
pub(crate) const SHOWCASE: &str = "material Paint : surface budget 1 { param color : ColorLinear = linear(0.18, 0.42, 0.72); param roughness : float = 0.35; let tiling : vec2 = uv(); let scale : float = 6; let scaled : vec2 = mul(tiling, scale); let grain : float = noise(scaled); let dark : ColorLinear = linear(0.03, 0.05, 0.08); let tinted : ColorLinear = lerp(dark, color, grain); let n : vec3 = worldNormal(); let v : vec3 = viewDirection(); let power : float = 3; let rim : float = fresnel(n, v, power); output baseColor = tinted; output roughness = roughness; output clearCoat = rim; }";

/// In-node preview bookkeeping for the open material (one per shell).
#[derive(Default)]
pub(crate) struct NodePreviews {
    /// The document and graph revision the requests were derived for.
    pub revision: Option<(String, u64)>,
    /// Node ID to its preview's cache key, derived material and shot.
    pub requests: BTreeMap<String, (String, Material, Shot)>,
    /// Hidden by the toolbar toggle (`material.node_previews`).
    pub hidden: bool,
}

/// The preview material and shot for `node`, or `None` when it has no visual value
/// (parameters, constants, textures, matrices) or the derived material does not check.
pub(crate) fn derive(material: &Material, node: &str) -> Option<(Material, Shot)> {
    let binding = material.bindings.get(node)?;
    let Expr::Call { op, .. } = &binding.expr else {
        return None;
    };
    if op.starts_with("custom.") {
        return None;
    }
    let mut preview = material.clone();
    preview.domain = Domain::Unlit;
    preview.outputs.clear();
    let mut added = Added {
        material: &mut preview,
        next: 0,
    };
    let node = node.to_owned();
    let color = match binding.ty {
        Type::ColorLinear => node,
        Type::ColorSrgb => added.call(Type::ColorLinear, "linear", [node]),
        Type::Vec3 => added.call(Type::ColorLinear, "color", [node]),
        Type::Vec4 => {
            let rgb = added.call(Type::Vec3, "rgb", [node]);
            added.call(Type::ColorLinear, "color", [rgb])
        }
        Type::Float | Type::Bool => {
            let value = if binding.ty == Type::Bool {
                added.call(Type::Float, "float", [node])
            } else {
                node
            };
            let white = added.literal(Type::Vec3, &[1., 1., 1.]);
            let grey = added.call(Type::Vec3, "mul", [white, value]);
            added.call(Type::ColorLinear, "color", [grey])
        }
        Type::Vec2 => {
            // x to red and y to green (UVs read as the familiar red-green square).
            let mut lane = |axis: [f64; 2], channel: [f64; 3]| {
                let pick = added.literal(Type::Vec2, &axis);
                let value = added.call(Type::Float, "dot", [node.clone(), pick]);
                let tint = added.literal(Type::Vec3, &channel);
                added.call(Type::Vec3, "mul", [tint, value])
            };
            let red = lane([1., 0.], [1., 0., 0.]);
            let green = lane([0., 1.], [0., 1., 0.]);
            let both = added.call(Type::Vec3, "add", [red, green]);
            added.call(Type::ColorLinear, "color", [both])
        }
        _ => return None,
    };
    preview.outputs.insert("color".into(), color);
    preview.normalize_order().ok()?;
    ara_material::generate(&preview, &Default::default()).ok()?;
    let shape = if world_space(material, &binding.expr) {
        Shape::Sphere
    } else {
        Shape::Swatch
    };
    Some((preview, Shot { shape, data: true }))
}

/// Bindings appended to the derived material, with fresh `n_preview*` identities.
struct Added<'a> {
    material: &'a mut Material,
    next: u32,
}
impl Added<'_> {
    fn insert(&mut self, ty: Type, expr: Expr) -> String {
        let id = format!("n_preview{}", self.next);
        self.material.bindings.insert(
            id.clone(),
            Binding {
                name: format!("preview_{}", self.next),
                order: u32::MAX - 64 + self.next,
                ty,
                expr,
            },
        );
        self.next += 1;
        id
    }
    fn call<const N: usize>(&mut self, ty: Type, op: &str, args: [String; N]) -> String {
        self.insert(
            ty,
            Expr::Call {
                op: op.into(),
                args: args.into(),
            },
        )
    }
    fn literal(&mut self, ty: Type, values: &[f64]) -> String {
        self.insert(
            ty,
            Expr::Literal {
                values: values.into(),
            },
        )
    }
}

/// Whether an expression reads world position, normal or view direction (directly or
/// through its arguments), which only reads correctly on 3D geometry.
fn world_space(material: &Material, expr: &Expr) -> bool {
    let mut stack = vec![expr];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(expr) = stack.pop() {
        let Expr::Call { op, args } = expr else {
            continue;
        };
        if matches!(
            op.as_str(),
            "worldPosition" | "worldNormal" | "viewDirection"
        ) {
            return true;
        }
        for arg in args {
            if seen.insert(arg)
                && let Some(binding) = material.bindings.get(arg)
            {
                stack.push(&binding.expr);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(source: &str) -> Material {
        ara_material::dsl::parse(source, "gr_preview").expect("material")
    }

    #[test]
    fn operations_preview_as_unlit_colour_and_values_do_not() {
        let m = material(
            "material Grain : surface budget 1 { param tint : ColorLinear = linear(0.8, 0.4, 0.1); let tiling : vec2 = uv(); let grain : float = noise(tiling); let n : vec3 = worldNormal(); let v : vec3 = viewDirection(); let power : float = 2; let facing : float = fresnel(n, v, power); output baseColor = tint; output roughness = grain; output metallic = facing; }",
        );
        let id = |name: &str| m.node_id(name).expect("node").to_owned();
        let (uv, shot) = derive(&m, &id("tiling")).expect("uv preview");
        assert_eq!(uv.domain, Domain::Unlit);
        assert_eq!(shot.shape, Shape::Swatch);
        assert!(shot.data);
        let (_, shot) = derive(&m, &id("grain")).expect("noise preview");
        assert_eq!(shot.shape, Shape::Swatch);
        let (_, shot) = derive(&m, &id("facing")).expect("fresnel preview");
        assert_eq!(shot.shape, Shape::Sphere);
        assert!(derive(&m, &id("tint")).is_none(), "parameters show values");
    }

    /// The capture route's showcase material parses, checks and previews every operation.
    #[test]
    fn the_route_material_previews_its_operations() {
        let m = material(SHOWCASE);
        ara_material::check(&m).expect("checks");
        let previews = m
            .bindings
            .keys()
            .filter(|id| derive(&m, id).is_some())
            .count();
        assert_eq!(previews, 7, "uv, scaled, grain, tinted, n, v, rim");
    }
}
