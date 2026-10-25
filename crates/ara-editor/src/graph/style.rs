//! Domain presentation for canvas nodes: human titles, category accents, inline values
//! and pin labels. Registry ids, port keys and DSL names stay in reports and the
//! Inspector; the canvas shows what a person reads (UE/Shader Graph conventions).
use ara_graph::{Direction, Graph, Node};
use ara_ui::style::Color;
use serde_json::Value;

/// How a node draws: titled cards with typed pins, or state-machine states.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Shape {
    Pins,
    State,
}

/// Header accent, resolved against the theme (chart tokens keep both themes legible).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Accent {
    Neutral,
    Primary,
    /// `chart-1`..`chart-6`, zero-based.
    Chart(usize),
}

/// One node's presentation, derived on model revisions only.
pub(super) struct Look {
    pub title: String,
    pub caption: String,
    pub value: Option<String>,
    pub swatch: Option<Color>,
    pub accent: Accent,
    pub shape: Shape,
    /// The state machine's default (entry) state.
    pub initial: bool,
    /// Reserves an in-node preview of the value (material operations, Shader Graph style).
    pub preview: bool,
}

/// Presentation of a node in its graph's domain.
pub(super) fn look(graph: &Graph, id: &str, node: &Node) -> Look {
    let name = node
        .payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(id)
        .to_owned();
    if let Some(op) = node.kind.strip_prefix("material.") {
        return material(graph, op, name, node);
    }
    if let Some(kind) = node.kind.strip_prefix("animation.") {
        return animation(kind, name, node);
    }
    Look {
        title: name,
        caption: node.kind.clone(),
        value: None,
        swatch: None,
        accent: Accent::Neutral,
        shape: Shape::Pins,
        initial: false,
        preview: false,
    }
}

fn material(graph: &Graph, op: &str, name: String, node: &Node) -> Look {
    let ty = node
        .payload
        .get("ty")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let value = node.params.get("value");
    let lanes = value
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect::<Vec<_>>());
    let colour = ty.starts_with("Color");
    let swatch = lanes
        .as_ref()
        .filter(|lanes| colour && lanes.len() >= 3)
        .map(|lanes| swatch(lanes, ty == "ColorLinear"));
    let value_text = match op {
        "parameter" | "literal" => value.map(|v| match v {
            Value::Array(_) => lanes
                .as_deref()
                .map(lanes_text)
                .unwrap_or_else(|| "…".into()),
            Value::Bool(b) => b.to_string(),
            Value::String(s) => file_name(s),
            Value::Object(o) => o
                .values()
                .find_map(Value::as_str)
                .map(file_name)
                .unwrap_or_else(|| "texture".into()),
            other => other.to_string(),
        }),
        _ => None,
    };
    let accent = category(op).1;
    let caption = match op {
        "parameter" => short_type(&ty),
        "literal" => "Constant".into(),
        "output" => String::new(),
        "custom" => node
            .payload
            .get("function")
            .and_then(Value::as_str)
            .unwrap_or("Custom")
            .to_owned(),
        _ => op_title(op),
    };
    let title = if op == "output" {
        let domain = graph
            .header
            .payload
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or("surface");
        format!("{} output", sentence(domain))
    } else {
        name
    };
    Look {
        title,
        caption,
        value: value_text,
        swatch,
        accent,
        shape: Shape::Pins,
        initial: false,
        // Operations whose value can be seen; parameters and constants show values.
        preview: !matches!(op, "parameter" | "literal" | "output" | "custom")
            && matches!(
                ty.as_str(),
                "float" | "bool" | "vec2" | "vec3" | "vec4" | "ColorLinear" | "ColorSRGB"
            ),
    }
}

fn animation(kind: &str, name: String, node: &Node) -> Look {
    let pose = node
        .payload
        .get("pose")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    match kind {
        "entry" | "state" => Look {
            title: name,
            caption: pose,
            value: None,
            swatch: None,
            accent: if kind == "entry" {
                Accent::Primary
            } else {
                Accent::Neutral
            },
            shape: Shape::State,
            initial: kind == "entry",
            preview: false,
        },
        _ => Look {
            title: name,
            caption: match kind {
                "Clip" => "Clip".into(),
                "Blend1d" => "1D blend".into(),
                "Directional2d" => "2D blend".into(),
                "MotionMatching" => "Motion matching".into(),
                other => sentence(other),
            },
            value: None,
            swatch: None,
            accent: if kind == "Clip" {
                Accent::Chart(1)
            } else {
                Accent::Chart(3)
            },
            shape: Shape::Pins,
            initial: false,
            preview: false,
        },
    }
}

/// Material node categories in palette order, with their header accents.
pub(crate) const CATEGORIES: [&str; 6] =
    ["Inputs", "Math", "Texture", "Color", "Utility", "Output"];

/// A material operation's category and accent (shared by canvas headers and Add node).
pub(crate) fn category(op: &str) -> (&'static str, Accent) {
    match op {
        "parameter" => ("Inputs", Accent::Chart(1)),
        "literal" => ("Inputs", Accent::Chart(5)),
        "uv" | "worldPosition" | "worldNormal" | "viewDirection" | "time" => {
            ("Inputs", Accent::Chart(2))
        }
        "sampleColor" | "sampleNormal" | "sampleData" | "triplanar" | "parallax" | "sceneColor" => {
            ("Texture", Accent::Chart(3))
        }
        "linear" | "srgb" | "color" | "rgb" | "alpha" | "luminance" => ("Color", Accent::Chart(4)),
        "copy" | "float" | "normalBlend" | "normalStrength" | "noise" | "fresnel" | "rotateUV"
        | "roundedPanel" | "custom" => ("Utility", Accent::Neutral),
        "output" => ("Output", Accent::Primary),
        _ => ("Math", Accent::Neutral),
    }
}

/// A pin's label: argument names for material calls, field names on outputs,
/// sample names on blends, and the value type on single outputs.
pub(crate) fn pin_label(node: &Node, port: &str, direction: Direction, ty: &str) -> String {
    if direction == Direction::Output {
        return match port {
            "pose" => "Pose".into(),
            "out" => short_type(ty),
            other => sentence(other),
        };
    }
    if let Some(index) = port
        .strip_prefix("in")
        .and_then(|i| i.parse::<usize>().ok())
    {
        let op = node.kind.strip_prefix("material.").unwrap_or("");
        return arguments(op)
            .get(index)
            .map_or_else(|| format!("Input {}", index + 1), |name| (*name).into());
    }
    if let Some(index) = port
        .strip_prefix("sample")
        .and_then(|i| i.parse::<usize>().ok())
    {
        if let Some(sample) = node
            .payload
            .get("samples")
            .and_then(Value::as_array)
            .and_then(|samples| samples.get(index))
        {
            let child = sample.get("node").and_then(Value::as_str).unwrap_or("");
            let x = sample.get("x").and_then(Value::as_f64).unwrap_or(0.);
            return format!("{child} · {}", number(x));
        }
        return format!("Sample {}", index + 1);
    }
    sentence(port)
}

/// Argument names of material built-ins, in signature order.
fn arguments(op: &str) -> &'static [&'static str] {
    match op {
        "add" | "sub" | "mul" | "div" | "min" | "max" | "dot" | "cross" => &["A", "B"],
        "clamp" => &["Value", "Min", "Max"],
        "lerp" => &["A", "B", "Alpha"],
        "pow" => &["Base", "Exponent"],
        "step" => &["Edge", "Value"],
        "smoothstep" => &["Lower", "Upper", "Value"],
        "sampleColor" | "sampleNormal" | "sampleData" => &["Texture", "UV"],
        "triplanar" => &["Texture", "Position", "Normal", "Scale"],
        "parallax" => &["Height", "UV", "View", "Scale"],
        "fresnel" => &["Normal", "View", "Power"],
        "rotateUV" => &["UV", "Radians"],
        "transform" | "transform3" => &["Matrix", "Vector"],
        "normalBlend" => &["Base", "Detail"],
        "normalStrength" => &["Normal", "Strength"],
        "noise" | "sceneColor" => &["UV"],
        "roundedPanel" => &["UV", "Size", "Radius"],
        "luminance" | "linear" => &["Color"],
        "srgb" | "color" => &["RGB"],
        "rgb" | "alpha" => &["RGBA"],
        "float" => &["Bool"],
        "normalize" | "length" => &["Vector"],
        _ => &["X"],
    }
}

/// Human operation names ("mul" → "Multiply", "sampleColor" → "Sample color").
pub(crate) fn op_title(op: &str) -> String {
    match op {
        "mul" => "Multiply".into(),
        "sub" => "Subtract".into(),
        "div" => "Divide".into(),
        "uv" => "UV".into(),
        "rotateUV" => "Rotate UV".into(),
        "srgb" => "sRGB".into(),
        "rgb" => "RGB".into(),
        "fract" => "Fraction".into(),
        "lerp" => "Lerp".into(),
        "float" => "To float".into(),
        "transform3" => "Transform (3×3)".into(),
        other => sentence(other),
    }
}

/// Compact type names: colours and textures read as words, vectors stay terse.
pub(crate) fn short_type(ty: &str) -> String {
    match ty {
        "ColorLinear" => "Color".into(),
        "ColorSRGB" | "ColorSrgb" => "sRGB color".into(),
        "TextureColor" => "Texture".into(),
        "TextureNormal" => "Normal map".into(),
        "TextureData" => "Data texture".into(),
        other => other.into(),
    }
}

/// `baseColor` / `clear_coat` → "Base color" / "Clear coat".
pub(crate) fn sentence(key: &str) -> String {
    let mut out = String::new();
    for (i, c) in key.chars().enumerate() {
        if c == '_' || c == '-' || c == '.' {
            out.push(' ');
        } else if c.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn number(v: f64) -> String {
    if v.fract() == 0. {
        format!("{v:.0}")
    } else {
        let text = format!("{v:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

fn lanes_text(lanes: &[f64]) -> String {
    lanes
        .iter()
        .map(|v| number(*v))
        .collect::<Vec<_>>()
        .join(", ")
}

fn file_name(reference: &str) -> String {
    reference
        .trim_start_matches('@')
        .split('#')
        .next()
        .and_then(|p| p.rsplit('/').next())
        .unwrap_or(reference)
        .to_owned()
}

/// Display colour of colour lanes; linear lanes are encoded to sRGB.
pub(super) fn swatch(lanes: &[f64], linear: bool) -> Color {
    let channel = |i: usize| {
        let v = lanes.get(i).copied().unwrap_or(0.).clamp(0., 1.);
        let v = if linear { v.powf(1. / 2.2) } else { v };
        (v * 255.).round() as u8
    };
    Color(channel(0), channel(1), channel(2), 255)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_read_operations_arguments_and_fields() {
        assert_eq!(op_title("mul"), "Multiply");
        assert_eq!(op_title("sampleColor"), "Sample color");
        assert_eq!(sentence("baseColor"), "Base color");
        assert_eq!(arguments("lerp"), &["A", "B", "Alpha"]);
        assert_eq!(short_type("ColorLinear"), "Color");
        assert_eq!(number(0.35), "0.35");
        assert_eq!(number(2.), "2");
    }
}
