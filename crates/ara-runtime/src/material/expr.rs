//! Output-reachable expressions lower to constants, affine UVs or one texture sample.
use super::{TextureGeneration, uv::Affine};
use crate::{
    Result, RuntimeError,
    assets::{AssetSource, asset_id},
    model::{ModelTexture, TextureTransform, TextureUsage},
};
use ara_asset::ArtifactKind;
use ara_asset_format::{Texture, TextureFormat};
use ara_material::{Expr, Material, ParameterValue, TextureDefault, TextureSource};
use glam::{Vec2, Vec3};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SampleKind {
    Color,
    Data,
    Normal,
}

#[derive(Debug, Clone)]
pub(super) struct Sample {
    pub texture: usize,
    pub uv: TextureTransform,
    pub channels: Vec<usize>,
    pub factors: Vec<f32>,
    pub kind: SampleKind,
    pub normal_scale: f32,
}

/// One vertex-colour channel term per channel R, G, B, A of a paint-layer weight:
/// 0 unused, 1 the channel `c`, 2 `1 - c` (ara_render::scene::PaintWeight).
pub(super) type Terms = [u8; 4];

#[derive(Debug, Clone)]
pub(super) enum Signal {
    Constant(Vec<f32>),
    Coordinates(Affine),
    Texture(TextureSource),
    Sample(Sample),
    /// Lanes of the interpolated vertex colour (`vertexColor()`), R first.
    Vertex(Vec<usize>),
    /// A scalar vertex-colour weight: channel `c` (or `1 - c`, inverted).
    Mask {
        channel: usize,
        inverted: bool,
    },
    /// A paint-layer blend (`lerp` by vertex channels), flattened: each term's
    /// signal weighted by the product of its channel terms; the weights sum to one.
    Layered(Vec<(Terms, Signal)>),
    /// A signal multiplied by the vertex colour (base colour and opacity only).
    Tinted(Box<Signal>),
}

/// The product of two layer weights, or `None` when they contradict (one takes a
/// channel, the other its complement).
pub(super) fn merge(a: Terms, b: Terms) -> Option<Terms> {
    let mut out = a;
    for c in 0..4 {
        match (a[c], b[c]) {
            (_, 0) => {}
            (0, term) => out[c] = term,
            (x, y) if x == y => {}
            _ => return None,
        }
    }
    Some(out)
}

pub(super) struct Evaluator<'a> {
    material: &'a Material,
    source: &'a dyn AssetSource,
    values: BTreeMap<String, Signal>,
    loaded: BTreeMap<String, usize>,
    pub textures: Vec<ModelTexture>,
    pub generations: Vec<TextureGeneration>,
}

impl<'a> Evaluator<'a> {
    pub fn new(material: &'a Material, source: &'a dyn AssetSource) -> Self {
        Self {
            material,
            source,
            values: BTreeMap::new(),
            loaded: BTreeMap::new(),
            textures: Vec::new(),
            generations: Vec::new(),
        }
    }

    pub fn bad(&self, node: &str, reason: &str) -> RuntimeError {
        RuntimeError(format!("{}#{node}: {reason}", self.material.id))
    }

    pub fn output(&mut self, name: &str, default: &[f32]) -> Result<(String, Signal)> {
        if let Some(id) = self.material.outputs.get(name) {
            Ok((id.clone(), self.value(id)?))
        } else {
            Ok((name.to_owned(), Signal::Constant(default.to_vec())))
        }
    }

    fn value(&mut self, id: &str) -> Result<Signal> {
        if let Some(value) = self.values.get(id) {
            return Ok(value.clone());
        }
        let material = self.material;
        let value = if let Some(parameter) = material.parameters.get(id) {
            match &parameter.default {
                ParameterValue::Texture(source) => Signal::Texture(source.clone()),
                _ => return Err(self.bad(id, "numeric parameter was not specialized")),
            }
        } else {
            let binding = material
                .bindings
                .get(id)
                .ok_or_else(|| self.bad(id, "missing material node"))?;
            match &binding.expr {
                Expr::Literal { values } => {
                    Signal::Constant(values.iter().map(|v| *v as f32).collect())
                }
                Expr::Call { op, args } => self.call(id, op, args)?,
            }
        };
        self.values.insert(id.to_owned(), value.clone());
        Ok(value)
    }

    fn call(&mut self, id: &str, op: &str, args: &[String]) -> Result<Signal> {
        match (op, args) {
            ("uv", []) => Ok(Signal::Coordinates(Affine::identity())),
            ("vertexColor", []) => Ok(Signal::Vertex(vec![0, 1, 2, 3])),
            ("red" | "green" | "blue", [value]) => {
                let lane = match op {
                    "red" => 0,
                    "green" => 1,
                    _ => 2,
                };
                match self.value(value)? {
                    Signal::Constant(values) => Ok(Signal::Constant(vec![values[lane]])),
                    Signal::Vertex(lanes) => Ok(mask(lanes[lane])),
                    Signal::Sample(mut sample) if sample.kind != SampleKind::Normal => {
                        sample.channels = vec![sample.channels[lane]];
                        sample.factors = vec![sample.factors[lane]];
                        Ok(Signal::Sample(sample))
                    }
                    _ => Err(self.bad(id, "channel selection cannot be represented here")),
                }
            }
            ("saturate", [value]) => match self.value(value)? {
                // Vertex colours are already within 0..1.
                signal @ (Signal::Vertex(_) | Signal::Mask { .. }) => Ok(signal),
                Signal::Constant(values) => Ok(Signal::Constant(
                    values.into_iter().map(|v| v.clamp(0., 1.)).collect(),
                )),
                _ => Err(self.bad(id, "saturate applies to constants and vertex colours here")),
            },
            ("lerp", [a, b, t]) => {
                let (a, b, t) = (self.value(a)?, self.value(b)?, self.value(t)?);
                let (channel, inverted) = match t {
                    Signal::Mask { channel, inverted } => (channel, inverted),
                    Signal::Vertex(lanes) if lanes.len() == 1 => (lanes[0], false),
                    _ => {
                        return Err(self.bad(
                            id,
                            "lerp between material layers needs a vertex-colour channel weight, e.g. red(vertexColor())",
                        ));
                    }
                };
                let (weight, complement) = if inverted { (2, 1) } else { (1, 2) };
                let mut terms = Vec::new();
                for (side, signal) in [(complement, a), (weight, b)] {
                    for (mut weights, leaf) in layered(signal) {
                        if weights[channel] != 0 {
                            return Err(self
                                .bad(id, "a vertex channel may blend each layer path only once"));
                        }
                        weights[channel] = side;
                        terms.push((weights, leaf));
                    }
                }
                Ok(Signal::Layered(terms))
            }
            ("copy" | "color" | "srgb" | "float", [value]) => self.value(value),
            ("sampleColor" | "sampleData" | "sampleNormal", [texture, uv]) => {
                let Signal::Texture(source) = self.value(texture)? else {
                    return Err(self.bad(id, "expected a symbolic texture"));
                };
                let coordinates = self.value(uv)?;
                let uv = self
                    .coordinates(id, coordinates)?
                    .texture(&self.material.id, id)?;
                let kind = match op {
                    "sampleColor" => SampleKind::Color,
                    "sampleNormal" => SampleKind::Normal,
                    _ => SampleKind::Data,
                };
                match source {
                    TextureSource::Default(default) => {
                        Ok(Signal::Constant(fallback(default, kind)))
                    }
                    TextureSource::Asset(reference) => {
                        let texture = self.texture(&reference)?;
                        if kind == SampleKind::Normal
                            && self.textures[texture].usage != TextureUsage::Normal
                        {
                            return Err(self.bad(id, "sampleNormal requires a normal texture import; configure its usage as normal"));
                        }
                        let count = if kind == SampleKind::Data { 4 } else { 3 };
                        Ok(Signal::Sample(Sample {
                            texture,
                            uv,
                            channels: (0..count).collect(),
                            factors: vec![1.; count],
                            kind,
                            normal_scale: 1.,
                        }))
                    }
                }
            }
            ("rgb" | "alpha", [value]) => {
                let value = self.value(value)?;
                let lanes: &[usize] = if op == "rgb" { &[0, 1, 2] } else { &[3] };
                match value {
                    Signal::Vertex(vertex) if vertex.len() == 4 && op == "alpha" => Ok(mask(3)),
                    Signal::Vertex(vertex) if vertex.len() == 4 => {
                        Ok(Signal::Vertex(lanes.iter().map(|i| vertex[*i]).collect()))
                    }
                    Signal::Constant(values) => {
                        Ok(Signal::Constant(lanes.iter().map(|i| values[*i]).collect()))
                    }
                    Signal::Sample(mut sample) if sample.channels.len() == 4 => {
                        sample.channels = lanes.iter().map(|i| sample.channels[*i]).collect();
                        sample.factors = lanes.iter().map(|i| sample.factors[*i]).collect();
                        Ok(Signal::Sample(sample))
                    }
                    _ => Err(self.bad(
                        id,
                        "channel selection cannot be represented by this PBR slot",
                    )),
                }
            }
            ("linear", [value]) => match self.value(value)? {
                Signal::Constant(values) => {
                    Ok(Signal::Constant(values.into_iter().map(linear).collect()))
                }
                Signal::Sample(mut sample)
                    if sample.channels.len() == 3 && sample.factors.iter().all(|v| *v == 1.) =>
                {
                    sample.kind = SampleKind::Color;
                    Ok(Signal::Sample(sample))
                }
                _ => Err(self.bad(
                    id,
                    "sRGB conversion after arithmetic cannot be represented exactly",
                )),
            },
            ("rotateUV", [uv, angle]) => {
                let uv = self.value(uv)?;
                let angle = self.value(angle)?;
                let angle = self.scalar(id, angle)?;
                Ok(Signal::Coordinates(self.coordinates(id, uv)?.rotate(angle)))
            }
            ("normalStrength", [normal, strength]) => {
                let normal = self.value(normal)?;
                let strength = self.value(strength)?;
                let strength = self.scalar(id, strength)?;
                match normal {
                    Signal::Sample(mut sample) if sample.kind == SampleKind::Normal => {
                        sample.normal_scale *= strength;
                        Ok(Signal::Sample(sample))
                    }
                    Signal::Constant(values) if values.len() == 3 => {
                        let normal =
                            Vec3::new(values[0] * strength, values[1] * strength, values[2])
                                .normalize_or(Vec3::Z);
                        Ok(Signal::Constant(normal.to_array().to_vec()))
                    }
                    _ => Err(self.bad(
                        id,
                        "normalStrength needs a normal sample and constant strength",
                    )),
                }
            }
            ("normalize", [value]) => match self.value(value)? {
                Signal::Sample(sample) if sample.kind == SampleKind::Normal => {
                    Ok(Signal::Sample(sample))
                }
                _ => Err(self.bad(id, "only sampled normal normalization is representable")),
            },
            ("dot", [a, b]) => {
                let (a, b) = (self.value(a)?, self.value(b)?);
                // A one-hot selector picks a vertex-colour channel.
                if let (Signal::Vertex(lanes), Signal::Constant(values))
                | (Signal::Constant(values), Signal::Vertex(lanes)) = (&a, &b)
                {
                    let picked: Vec<_> = values
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| **v != 0.)
                        .collect();
                    return match picked.as_slice() {
                        [(index, 1.)] if values.len() == lanes.len() => Ok(mask(lanes[*index])),
                        _ => Err(self.bad(
                            id,
                            "only a one-hot selector of a vertex-colour channel is representable",
                        )),
                    };
                }
                let (mut sample, values) = match (a, b) {
                    (Signal::Sample(sample), Signal::Constant(values))
                    | (Signal::Constant(values), Signal::Sample(sample)) => (sample, values),
                    _ => {
                        return Err(
                            self.bad(id, "dot needs one sample and a constant channel selector")
                        );
                    }
                };
                if sample.kind == SampleKind::Normal || values.len() != sample.channels.len() {
                    return Err(self.bad(id, "invalid sampled channel selector"));
                }
                let mut nonzero = values.iter().enumerate().filter(|(_, v)| **v != 0.);
                let Some((index, factor)) = nonzero.next() else {
                    return Ok(Signal::Constant(vec![0.]));
                };
                if nonzero.next().is_some() {
                    return Err(self.bad(id, "combining multiple texture channels is not representable by one PBR channel"));
                }
                sample.channels = vec![sample.channels[index]];
                sample.factors = vec![sample.factors[index] * factor];
                Ok(Signal::Sample(sample))
            }
            ("add" | "sub" | "mul" | "div", [a, b]) => {
                let (a, b) = (self.value(a)?, self.value(b)?);
                match (op, a, b) {
                    // One minus a vertex channel: the complementary weight.
                    ("sub", Signal::Constant(one), Signal::Vertex(lanes))
                        if one.as_slice() == [1.] && lanes.len() == 1 =>
                    {
                        Ok(Signal::Mask {
                            channel: lanes[0],
                            inverted: true,
                        })
                    }
                    ("sub", Signal::Constant(one), Signal::Mask { channel, inverted })
                        if one.as_slice() == [1.] =>
                    {
                        Ok(Signal::Mask {
                            channel,
                            inverted: !inverted,
                        })
                    }
                    // Base colour times the vertex colour (glTF COLOR_0 semantics).
                    ("mul", Signal::Vertex(lanes), other)
                    | ("mul", other, Signal::Vertex(lanes))
                        if lanes.len() >= 3 && lanes[..3] == [0, 1, 2] =>
                    {
                        Ok(Signal::Tinted(Box::new(other)))
                    }
                    (_, a, b) => self.binary(id, op, a, b),
                }
            }
            _ => Err(self.bad(
                id,
                &format!("unsupported reachable material operation `{op}`"),
            )),
        }
    }

    pub fn scalar(&self, id: &str, value: Signal) -> Result<f32> {
        match value {
            Signal::Constant(values) if values.len() == 1 && values[0].is_finite() => Ok(values[0]),
            _ => Err(self.bad(id, "expected a finite constant scalar")),
        }
    }

    fn coordinates(&self, id: &str, value: Signal) -> Result<Affine> {
        match value {
            Signal::Coordinates(uv) => Ok(uv),
            Signal::Constant(values) if values.len() == 2 => {
                Ok(Affine::constant(Vec2::new(values[0], values[1])))
            }
            _ => Err(self.bad(id, "expected affine primary UV coordinates")),
        }
    }

    fn binary(&self, id: &str, op: &str, a: Signal, b: Signal) -> Result<Signal> {
        let vertex = |s: &Signal| {
            matches!(
                s,
                Signal::Vertex(_) | Signal::Mask { .. } | Signal::Layered(_) | Signal::Tinted(_)
            )
        };
        if vertex(&a) || vertex(&b) {
            return Err(self.bad(
                id,
                "vertex colours lerp between layers by a channel (or its 1 - c), or multiply base colour; other arithmetic is not representable",
            ));
        }
        match (a, b) {
            (Signal::Constant(a), Signal::Constant(b)) => {
                let count = a.len().max(b.len());
                let (a, b) = (lanes(&a, count), lanes(&b, count));
                let (Some(a), Some(b)) = (a, b) else {
                    return Err(self.bad(id, "incompatible constant lanes"));
                };
                let values: Vec<_> = a
                    .into_iter()
                    .zip(b)
                    .map(|(a, b)| match op {
                        "add" => a + b,
                        "sub" => a - b,
                        "mul" => a * b,
                        _ => a / b,
                    })
                    .collect();
                if values.iter().any(|v| !v.is_finite()) {
                    return Err(self.bad(id, "non-finite material arithmetic"));
                }
                Ok(Signal::Constant(values))
            }
            (Signal::Coordinates(uv), Signal::Constant(values)) => {
                let values = lanes(&values, 2)
                    .ok_or_else(|| self.bad(id, "UV arithmetic needs scalar/vec2 constants"))?;
                let value = Vec2::new(values[0], values[1]);
                let uv = match op {
                    "add" => uv.translate(value),
                    "sub" => uv.translate(-value),
                    "mul" => uv.scale(value),
                    _ if value.x != 0. && value.y != 0. => uv.scale(value.recip()),
                    _ => return Err(self.bad(id, "division by zero in UV transform")),
                };
                Ok(Signal::Coordinates(uv))
            }
            (Signal::Constant(values), Signal::Coordinates(uv)) if op != "div" => {
                if op == "sub" {
                    let values =
                        lanes(&values, 2).ok_or_else(|| self.bad(id, "invalid UV lanes"))?;
                    Ok(Signal::Coordinates(
                        uv.scale(Vec2::NEG_ONE)
                            .translate(Vec2::new(values[0], values[1])),
                    ))
                } else {
                    self.binary(id, op, Signal::Coordinates(uv), Signal::Constant(values))
                }
            }
            (Signal::Sample(mut sample), Signal::Constant(values)) => {
                let values = lanes(&values, sample.channels.len())
                    .ok_or_else(|| self.bad(id, "incompatible sample multiplier lanes"))?;
                if sample.kind == SampleKind::Normal
                    || ((op == "add" || op == "sub") && values.iter().any(|v| *v != 0.))
                {
                    return Err(self.bad(
                        id,
                        "sample arithmetic cannot be represented by a PBR factor",
                    ));
                }
                for (factor, value) in sample.factors.iter_mut().zip(values) {
                    *factor = match op {
                        "mul" => *factor * value,
                        "div" => *factor / value,
                        _ => *factor,
                    };
                    if !factor.is_finite() {
                        return Err(self.bad(id, "non-finite texture multiplier"));
                    }
                }
                if sample.factors.iter().all(|v| *v == 0.) {
                    Ok(Signal::Constant(vec![0.; sample.factors.len()]))
                } else {
                    Ok(Signal::Sample(sample))
                }
            }
            (Signal::Constant(values), Signal::Sample(sample)) if op == "mul" || op == "add" => {
                self.binary(id, op, Signal::Sample(sample), Signal::Constant(values))
            }
            (Signal::Sample(mut a), Signal::Sample(b))
                if (op == "add" || op == "sub")
                    && a.kind != SampleKind::Normal
                    && a.kind == b.kind
                    && a.texture == b.texture
                    && a.uv == b.uv
                    && a.channels == b.channels =>
            {
                for (a, b) in a.factors.iter_mut().zip(b.factors) {
                    *a = if op == "add" { *a + b } else { *a - b };
                }
                Ok(Signal::Sample(a))
            }
            _ => Err(self.bad(
                id,
                "nonlinear samples or non-affine UVs cannot be represented by this PBR material",
            )),
        }
    }

    fn texture(&mut self, reference: &ara_schema::AssetRef) -> Result<usize> {
        let id = asset_id(reference)?;
        if let Some(index) = self.loaded.get(id) {
            return Ok(*index);
        }
        let (digest, cached) = self.source.load(id)?.ok_or_else(|| {
            RuntimeError(format!(
                "{}: texture {} is not imported",
                self.material.id, reference.0
            ))
        })?;
        let artifact = cached
            .output
            .artifacts
            .into_iter()
            .find(|a| a.kind == ArtifactKind::Texture && a.name == "texture.ktx2")
            .ok_or_else(|| {
                RuntimeError(format!(
                    "{}: {} is not a standalone texture import",
                    self.material.id, reference.0
                ))
            })?;
        let format = Texture::read(&artifact.bytes)
            .map_err(|e| RuntimeError(format!("{}: {e}", reference.0)))?
            .format();
        let usage = match format {
            TextureFormat::Bc5 => TextureUsage::Normal,
            TextureFormat::Bc7Srgb
            | TextureFormat::Astc4x4Srgb
            | TextureFormat::Astc6x6Srgb
            | TextureFormat::Astc8x8Srgb => TextureUsage::Color,
            _ => TextureUsage::Linear,
        };
        let index = self.textures.len();
        self.textures.push(ModelTexture {
            name: artifact.name,
            usage,
            ktx2: artifact.bytes.into(),
        });
        self.generations.push(TextureGeneration {
            asset: id.to_owned(),
            digest,
        });
        self.loaded.insert(id.to_owned(), index);
        Ok(index)
    }
}

/// A vertex channel as a scalar weight.
fn mask(channel: usize) -> Signal {
    Signal::Mask {
        channel,
        inverted: false,
    }
}

/// A signal's paint-layer terms: itself under no weight unless already layered.
pub(super) fn layered(signal: Signal) -> Vec<(Terms, Signal)> {
    match signal {
        Signal::Layered(terms) => terms,
        signal => vec![([0; 4], signal)],
    }
}

fn lanes(values: &[f32], count: usize) -> Option<Vec<f32>> {
    if values.len() == 1 {
        Some(vec![values[0]; count])
    } else if values.len() == count {
        Some(values.to_vec())
    } else {
        None
    }
}

fn linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn fallback(value: TextureDefault, kind: SampleKind) -> Vec<f32> {
    match kind {
        SampleKind::Normal => match value {
            TextureDefault::FlatNormal => vec![0., 0., 1.],
            TextureDefault::White => Vec3::ONE.normalize().to_array().to_vec(),
            TextureDefault::Black => (-Vec3::ONE.normalize()).to_array().to_vec(),
        },
        SampleKind::Color => match value {
            TextureDefault::White => vec![1.; 3],
            TextureDefault::Black => vec![0.; 3],
            TextureDefault::FlatNormal => vec![linear(0.5), linear(0.5), 1.],
        },
        SampleKind::Data => match value {
            TextureDefault::White => vec![1.; 4],
            TextureDefault::Black => vec![0., 0., 0., 1.],
            TextureDefault::FlatNormal => vec![0.5, 0.5, 1., 1.],
        },
    }
}
