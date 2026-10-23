use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

macro_rules! names {
    ($name:ident, $doc:literal, {$($(#[$meta:meta])* $variant:ident => $text:literal),* $(,)?}) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name { $($(#[$meta])* #[serde(rename=$text)] $variant),* }
        impl $name {
            /// All supported values in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];
            /// Canonical text/graph type name.
            pub fn name(self) -> &'static str { match self { $(Self::$variant => $text),* } }
        }
        impl fmt::Display for $name {
            fn fmt(&self,f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.name()) }
        }
        impl FromStr for $name {
            type Err = String;
            fn from_str(s:&str)->Result<Self,String>{ match s { $($text=>Ok(Self::$variant),)* _=>Err(format!("unknown {} `{s}`",stringify!($name))) } }
        }
    }
}

names!(Type, "Material value type, including color space and texture interpretation.", {
    /// Boolean switch; static parameters derive two variants from this type.
    Bool => "bool",
    /// Scalar IEEE shader float.
    Float => "float",
    /// Two scalar lanes.
    Vec2 => "vec2",
    /// Three non-color lanes (directions/data).
    Vec3 => "vec3",
    /// Four non-color lanes.
    Vec4 => "vec4",
    /// Row-major 3 by 3 matrix.
    Mat3 => "mat3",
    /// Row-major 4 by 4 matrix.
    Mat4 => "mat4",
    /// Encoded sRGB triple; arithmetic requires explicit conversion.
    ColorSrgb => "ColorSRGB",
    /// Scene-linear RGB triple.
    ColorLinear => "ColorLinear",
    /// Raw UNORM color texture; sampleColor explicitly decodes sRGB.
    TextureColor => "TextureColor",
    /// Tangent-space XYZ normal texture (RGB UNORM).
    TextureNormal => "TextureNormal",
    /// Linear, uninterpreted RGBA data texture.
    TextureData => "TextureData"
});

impl Type {
    /// Number of scalar lanes in a literal or packed parameter.
    pub fn lanes(self) -> usize {
        match self {
            Self::Bool
            | Self::Float
            | Self::TextureColor
            | Self::TextureNormal
            | Self::TextureData => 1,
            Self::Vec2 => 2,
            Self::Vec3 | Self::ColorSrgb | Self::ColorLinear => 3,
            Self::Vec4 => 4,
            Self::Mat3 => 9,
            Self::Mat4 => 16,
        }
    }
    /// Whether the value is a semantic texture resource.
    pub fn is_texture(self) -> bool {
        matches!(
            self,
            Self::TextureColor | Self::TextureNormal | Self::TextureData
        )
    }
    /// Arithmetic values exclude booleans, encoded colors and resource handles.
    pub fn is_numeric(self) -> bool {
        !self.is_texture() && !matches!(self, Self::ColorSrgb | Self::Bool)
    }
    /// Whether ordinary componentwise functions can act on this value.
    pub fn is_vector(self) -> bool {
        matches!(
            self,
            Self::Float | Self::Vec2 | Self::Vec3 | Self::Vec4 | Self::ColorLinear
        )
    }
    /// Slang storage/evaluation spelling; semantics remain in the AST.
    pub fn slang(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Float => "float",
            Self::Vec2 => "float2",
            Self::Vec3 | Self::ColorSrgb | Self::ColorLinear => "float3",
            Self::Vec4 => "float4",
            Self::Mat3 => "float3x3",
            Self::Mat4 => "float4x4",
            _ => "uint",
        }
    }
}

names!(Domain, "Compilation/output contract; UI never inherits surface lighting.", {
    /// Principled metallic/roughness surface with optional foliage transmission.
    Surface => "surface",
    /// Unlit scene geometry.
    Unlit => "unlit",
    /// Premultiplied linear UI with alpha and clipping.
    Ui => "ui",
    /// PBR decal values for the renderer's decal pass.
    Decal => "decal",
    /// Full-screen linear post-processing.
    PostProcess => "postProcess"
});

impl Domain {
    /// Canonical output order, independent of field-map ordering.
    pub fn output_order(self) -> &'static [&'static str] {
        match self {
            Self::Surface => &[
                "baseColor",
                "metallic",
                "roughness",
                "normal",
                "emissive",
                "opacityMask",
                "clearCoat",
                "clearCoatRoughness",
                "transmission",
            ],
            Self::Decal => &[
                "baseColor",
                "metallic",
                "roughness",
                "normal",
                "emissive",
                "opacity",
            ],
            Self::Unlit => &["color", "opacity", "opacityMask"],
            Self::Ui => &["color", "opacity", "clip"],
            Self::PostProcess => &["color", "opacity"],
        }
    }
    /// Allowed output fields and their exact semantic types.
    pub fn outputs(self) -> BTreeMap<&'static str, Type> {
        use Type::*;
        let fields: &[(&str, Type)] = match self {
            Self::Surface => &[
                ("baseColor", ColorLinear),
                ("metallic", Float),
                ("roughness", Float),
                ("normal", Vec3),
                ("emissive", ColorLinear),
                ("opacityMask", Float),
                ("clearCoat", Float),
                ("clearCoatRoughness", Float),
                ("transmission", Float),
            ],
            Self::Decal => &[
                ("baseColor", ColorLinear),
                ("metallic", Float),
                ("roughness", Float),
                ("normal", Vec3),
                ("emissive", ColorLinear),
                ("opacity", Float),
            ],
            Self::Unlit => &[
                ("color", ColorLinear),
                ("opacity", Float),
                ("opacityMask", Float),
            ],
            Self::Ui => &[("color", ColorLinear), ("opacity", Float), ("clip", Float)],
            Self::PostProcess => &[("color", ColorLinear), ("opacity", Float)],
        };
        fields.iter().copied().collect()
    }
}

/// A typed, named SSA-style expression; references are stable node IDs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Expr {
    /// Finite numeric lanes; booleans use zero/one internally, never texture slots.
    Literal {
        /// Row-major values, exactly `Type::lanes()` elements.
        values: Vec<f64>,
    },
    /// Pure built-in or opaque custom function call.
    Call {
        /// Registered function key, e.g. `mul` or `custom.coat`.
        op: String,
        /// Argument node IDs in signature order.
        args: Vec<String>,
    },
}

/// One named expression with explicit persistent semantic identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    /// Human/agent variable name; renaming does not move the graph node.
    pub name: String,
    /// Source tie-break order, normalized into dependency order on save.
    pub order: u32,
    /// Declared value type, verified before code generation.
    pub ty: Type,
    /// Literal or bounded expression.
    pub expr: Expr,
}

/// A definition parameter: runtime by default, or an explicit static boolean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Parameter {
    /// Variable name used by the text view.
    pub name: String,
    /// Author declaration order, independent of graph IDs.
    pub order: u32,
    /// Exact scalar/shape/resource semantics.
    pub ty: Type,
    /// Symbolic resource, boolean, or finite numeric default.
    pub default: ParameterValue,
    /// Whether a boolean parameter specializes the shader (two possible values).
    #[serde(default)]
    pub is_static: bool,
}

/// Built-in texture fallbacks, independent of renderer descriptor allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureDefault {
    /// Opaque white color/data texture.
    White,
    /// Opaque black color/data texture.
    Black,
    /// Tangent-space +Z normal texture.
    FlatNormal,
}

/// Authored texture source. Heap indices exist only in resolved GPU uploads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum TextureSource {
    /// Project-relative @path#id reference; path-only inputs resolve on save.
    Asset(ara_schema::AssetRef),
    /// Renderer-provided immutable fallback texture.
    Default(TextureDefault),
}

/// Typed authoring values for definition defaults and instance overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParameterValue {
    /// Numeric scalar/vector/matrix lanes in graph storage (DSL uses constructors).
    Numeric(Vec<f64>),
    /// A boolean value, including static switches.
    Bool(bool),
    /// A symbolic texture, never a descriptor slot.
    Texture(TextureSource),
}

impl ara_schema::FieldType for ParameterValue {
    fn value_type() -> ara_schema::ValueType {
        ara_schema::ValueType::Json
    }
}

impl ParameterValue {
    /// A valid zero/fallback for a declared material type.
    pub fn zero(ty: Type) -> Self {
        if ty == Type::Bool {
            Self::Bool(false)
        } else if ty.is_texture() {
            Self::Texture(TextureSource::Default(if ty == Type::TextureNormal {
                TextureDefault::FlatNormal
            } else {
                TextureDefault::White
            }))
        } else {
            Self::Numeric(vec![0.0; ty.lanes()])
        }
    }
}

/// Opaque Slang function with explicit typed I/O. Its body is never graphified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomFunction {
    /// Documentation shown on its graph node and in agent tools.
    pub doc: String,
    /// Ordered names and types, used verbatim as a Slang signature.
    pub inputs: Vec<(String, Type)>,
    /// Single typed result (use vector/matrix for compound numeric results).
    pub output: Type,
    /// Slang function body without the surrounding braces.
    pub body: String,
    /// Explicit upper estimate for opaque code; not a GPU measurement.
    pub cost: Cost,
}

/// Static estimates for reachable material work; actual GPU cost is renderer-owned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cost {
    /// Conservative texture sample count.
    pub texture_samples: u32,
    /// Scalar arithmetic operation estimate.
    pub alu: u32,
    /// Potential dynamic branches.
    pub branches: u32,
}

/// The single semantic material document. Graph layout is deliberately elsewhere.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Material {
    /// Graph asset ID from the containing file/index; omitted from material text.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Render/output domain.
    pub domain: Domain,
    /// Maximum declared static combinations, before pass/tier PSOs.
    pub variant_budget: u32,
    /// Runtime/static parameters keyed by persistent `n_...` identity.
    pub parameters: BTreeMap<String, Parameter>,
    /// Typed expression nodes keyed by persistent `n_...` identity.
    pub bindings: BTreeMap<String, Binding>,
    /// Opaque functions keyed by name (calls use `custom.<name>`).
    pub functions: BTreeMap<String, CustomFunction>,
    /// Domain field to expression/parameter node ID.
    pub outputs: BTreeMap<String, String>,
}

impl Material {
    /// Finds graph identity from its unique authored name.
    pub fn node_id(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(_, p)| p.name == name)
            .map(|(id, _)| id.as_str())
            .or_else(|| {
                self.bindings
                    .iter()
                    .find(|(_, b)| b.name == name)
                    .map(|(id, _)| id.as_str())
            })
    }
    /// Parameters in the author's declaration order, with stable ID as a fallback.
    pub fn ordered_parameters(&self) -> Vec<(&String, &Parameter)> {
        let mut values: Vec<_> = self.parameters.iter().collect();
        values.sort_by_key(|(id, p)| (p.order, *id));
        values
    }
    /// Normalizes declaration/dependency ordering after parsing or graph authoring.
    pub fn normalize_order(&mut self) -> Result<(), ara_diag::Diagnostic> {
        let params: Vec<_> = self
            .ordered_parameters()
            .iter()
            .map(|(id, _)| (*id).clone())
            .collect();
        for (i, id) in params.iter().enumerate() {
            if let Some(p) = self.parameters.get_mut(id) {
                p.order = i as u32;
            }
        }
        let order = crate::check::schedule(self)?;
        for (i, id) in order
            .iter()
            .filter(|id| self.bindings.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>()
            .iter()
            .enumerate()
        {
            if let Some(b) = self.bindings.get_mut(id) {
                b.order = i as u32;
            }
        }
        Ok(())
    }
    /// Resolves a node's declared semantic type.
    pub fn node_type(&self, id: &str) -> Option<Type> {
        self.bindings
            .get(id)
            .map(|n| n.ty)
            .or_else(|| self.parameters.get(id).map(|n| n.ty))
    }
    /// Resolves the display name of an expression/parameter node.
    pub fn node_name(&self, id: &str) -> Option<&str> {
        self.bindings
            .get(id)
            .map(|n| n.name.as_str())
            .or_else(|| self.parameters.get(id).map(|n| n.name.as_str()))
    }
}
