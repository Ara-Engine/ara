//! Starter operations share signature checking, graph metadata and cost estimates.

use ara_diag::Diagnostic;
use ara_graph::{NodeKind, NodeRegistry};

use crate::{Cost, Domain, Type};

/// Documented built-in expression available to both authoring views.
#[derive(Debug, Clone, Copy)]
pub struct Builtin {
    /// Stable DSL call name (registry kind is `material.<name>`).
    pub name: &'static str,
    /// Signature and intended use, exposed to agents and node search.
    pub doc: &'static str,
    /// Conservative static work estimate.
    pub cost: Cost,
}

macro_rules! nodes {
    ($($name:literal => ($samples:literal,$alu:literal,$branches:literal) $doc:literal),* $(,)?) => {
        /// Documented starter library; no generated-code-only nodes.
        pub const BUILTINS: &[Builtin] = &[$(Builtin{name:$name,doc:$doc,cost:Cost{texture_samples:$samples,alu:$alu,branches:$branches}}),*];
    }
}

nodes! {
    "add" => (0,4,0) "add(T,T) -> T: componentwise numeric addition.",
    "sub" => (0,4,0) "sub(T,T) -> T: componentwise numeric subtraction.",
    "mul" => (0,16,0) "mul(T,T|float) -> T: numeric multiplication; matrices use matrix multiplication.",
    "div" => (0,4,0) "div(T,T|float) -> T: componentwise division; caller must avoid zero divisors.",
    "min" => (0,4,0) "min(T,T) -> T: componentwise minimum of float/vector values.",
    "max" => (0,4,0) "max(T,T) -> T: componentwise maximum of float/vector values.",
    "clamp" => (0,8,0) "clamp(T,T,T) -> T: clamp each lane to its bounds.",
    "lerp" => (0,12,0) "lerp(T,T,float) -> T: linear interpolation in scene-linear space.",
    "abs" => (0,4,0) "abs(T) -> T: absolute magnitude per lane.",
    "sin" => (0,16,0) "sin(T) -> T: sine of radian float/vector lanes.",
    "cos" => (0,16,0) "cos(T) -> T: cosine of radian float/vector lanes.",
    "floor" => (0,4,0) "floor(T) -> T: round each lane toward negative infinity.",
    "fract" => (0,8,0) "fract(T) -> T: fractional lanes in [0,1).",
    "saturate" => (0,8,0) "saturate(T) -> T: clamp float/vector lanes to [0,1].",
    "sqrt" => (0,16,0) "sqrt(T) -> T: nonnegative square roots; avoid negative inputs.",
    "pow" => (0,24,0) "pow(T,T) -> T: componentwise power; use nonnegative bases.",
    "step" => (0,4,0) "step(T,T) -> T: threshold each lane (edge, value).",
    "smoothstep" => (0,24,0) "smoothstep(T,T,T) -> T: smooth threshold (lower, upper, value).",
    "dot" => (0,7,0) "dot(vecN,vecN) -> float: inner product for geometric vectors.",
    "cross" => (0,9,0) "cross(vec3,vec3) -> vec3: right-handed cross product.",
    "normalize" => (0,12,0) "normalize(vecN) -> vecN: unit direction with a safe zero fallback.",
    "length" => (0,12,0) "length(vecN) -> float: Euclidean vector length.",
    "uv" => (0,0,0) "uv() -> vec2: primary mesh UV or normalized UI/post coordinates.",
    "worldPosition" => (0,0,0) "worldPosition() -> vec3: world-space position; scene domains only.",
    "worldNormal" => (0,0,0) "worldNormal() -> vec3: interpolated world-space normal; scene domains only.",
    "viewDirection" => (0,0,0) "viewDirection() -> vec3: normalized surface-to-camera vector; scene domains only.",
    "time" => (0,0,0) "time() -> float: explicit engine frame phase from draw root constants, never CPU wall time.",
    "sampleColor" => (1,24,0) "sampleColor(TextureColor,vec2) -> ColorLinear: bilinear sample plus exact piecewise sRGB decode.",
    "sampleNormal" => (1,16,0) "sampleNormal(TextureNormal,vec2) -> vec3: RGB tangent-space unpack and safe normalization.",
    "sampleData" => (1,0,0) "sampleData(TextureData,vec2) -> vec4: uninterpreted linear RGBA sample.",
    "linear" => (0,24,0) "linear(ColorSRGB) -> ColorLinear: explicit piecewise sRGB decoding.",
    "srgb" => (0,0,0) "srgb(vec3) -> ColorSRGB: tag encoded color data without conversion.",
    "color" => (0,0,0) "color(vec3) -> ColorLinear: tag linear RGB data explicitly.",
    "rgb" => (0,0,0) "rgb(vec4) -> vec3: select the first three data lanes.",
    "alpha" => (0,0,0) "alpha(vec4) -> float: select the fourth data lane.",
    "luminance" => (0,5,0) "luminance(ColorLinear) -> float: Rec.709 luminance.",
    "normalBlend" => (0,24,0) "normalBlend(vec3,vec3) -> vec3: reoriented tangent normals with safe normalization.",
    "normalStrength" => (0,16,0) "normalStrength(vec3,float) -> vec3: scale tangent XY and renormalize.",
    "noise" => (0,80,0) "noise(vec2) -> float: bounded deterministic smooth value noise.",
    "triplanar" => (3,96,0) "triplanar(TextureColor,vec3,vec3,float) -> ColorLinear: world position/normal/scale color projection.",
    "parallax" => (1,24,0) "parallax(TextureData,vec2,vec3,float) -> vec2: bounded one-sample height offset (tangent view).",
    "fresnel" => (0,24,0) "fresnel(vec3,vec3,float) -> float: Schlick-style grazing factor from normal, view and power.",
    "rotateUV" => (0,16,0) "rotateUV(vec2,float) -> vec2: rotate about UV center by radians.",
    "transform" => (0,28,0) "transform(mat4,vec4) -> vec4: row-major matrix times column vector.",
    "transform3" => (0,15,0) "transform3(mat3,vec3) -> vec3: row-major matrix times geometric vector.",
    "roundedPanel" => (0,28,0) "roundedPanel(vec2,vec2,float) -> float: antialiased UI rounded-box coverage; UI domain only.",
    "sceneColor" => (1,0,0) "sceneColor(vec2) -> ColorLinear: sample explicit scene texture root index; post-process only."
}

/// Finds stable node metadata by DSL name.
pub fn get(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// Builds a searchable generic graph registry for every material node.
pub fn registry() -> Result<NodeRegistry, Diagnostic> {
    let mut registry = NodeRegistry::default();
    for (name, doc) in [
        (
            "parameter",
            "Typed material parameter, runtime or explicit static variant.",
        ),
        ("literal", "Finite typed scalar/vector/matrix constant."),
        ("output", "Typed domain output contract."),
        (
            "custom",
            "Opaque Slang function with explicit signature, documentation and cost.",
        ),
    ]
    .into_iter()
    .chain(BUILTINS.iter().map(|b| (b.name, b.doc)))
    {
        registry.add(NodeKind {
            name: format!("material.{name}"),
            version: 1,
            doc: doc.into(),
            ports: Default::default(),
            dynamic_ports: true,
        })?;
    }
    Ok(registry)
}

pub(crate) fn available(op: &str, domain: Domain) -> bool {
    match op {
        "roundedPanel" => domain == Domain::Ui,
        "sceneColor" => domain == Domain::PostProcess,
        "worldPosition" | "worldNormal" | "viewDirection" => {
            matches!(domain, Domain::Surface | Domain::Unlit | Domain::Decal)
        }
        _ => true,
    }
}

/// Resolves a built-in's polymorphic signature, rejecting implicit semantic casts.
pub fn infer(op: &str, a: &[Type]) -> Option<Type> {
    use Type::*;
    let same = || {
        a.first()
            .copied()
            .filter(|t| t.is_vector() && a.iter().all(|x| x == t))
    };
    match (op, a) {
        ("add" | "sub", [a, b]) if a == b && a.is_numeric() => Some(*a),
        ("mul", [a, b]) if a == b && a.is_numeric() => Some(*a),
        ("mul" | "div", [a, Float]) if a.is_numeric() => Some(*a),
        ("div", [a, b]) if a == b && a.is_vector() => Some(*a),
        ("min" | "max" | "pow" | "step", [_, _]) => same(),
        ("clamp" | "smoothstep", [_, _, _]) => same(),
        ("lerp", [a, b, Float]) if a == b && a.is_vector() => Some(*a),
        ("abs" | "sin" | "cos" | "floor" | "fract" | "saturate" | "sqrt", [_]) => same(),
        ("dot", [a, b]) if a == b && matches!(a, Vec2 | Vec3 | Vec4) => Some(Float),
        ("cross", [Vec3, Vec3]) => Some(Vec3),
        ("normalize", [a]) if matches!(a, Vec2 | Vec3 | Vec4) => Some(*a),
        ("length", [Vec2 | Vec3 | Vec4]) => Some(Float),
        ("uv", []) => Some(Vec2),
        ("time", []) => Some(Float),
        ("worldPosition" | "worldNormal" | "viewDirection", []) => Some(Vec3),
        ("sampleColor", [TextureColor, Vec2]) => Some(ColorLinear),
        ("sampleNormal", [TextureNormal, Vec2]) => Some(Vec3),
        ("sampleData", [TextureData, Vec2]) => Some(Vec4),
        ("linear", [ColorSrgb]) | ("color", [Vec3]) => Some(ColorLinear),
        ("srgb", [Vec3]) => Some(ColorSrgb),
        ("rgb", [Vec4]) => Some(Vec3),
        ("alpha", [Vec4]) => Some(Float),
        ("luminance", [ColorLinear]) => Some(Float),
        ("normalBlend", [Vec3, Vec3]) | ("normalStrength", [Vec3, Float]) => Some(Vec3),
        ("noise", [Vec2]) | ("roundedPanel", [Vec2, Vec2, Float]) => Some(Float),
        ("triplanar", [TextureColor, Vec3, Vec3, Float]) => Some(ColorLinear),
        ("parallax", [TextureData, Vec2, Vec3, Float]) | ("rotateUV", [Vec2, Float]) => Some(Vec2),
        ("fresnel", [Vec3, Vec3, Float]) => Some(Float),
        ("transform", [Mat4, Vec4]) => Some(Vec4),
        ("transform3", [Mat3, Vec3]) => Some(Vec3),
        ("sceneColor", [Vec2]) => Some(ColorLinear),
        _ => None,
    }
}
