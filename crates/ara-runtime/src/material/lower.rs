use super::{
    TextureGeneration,
    expr::{Evaluator, Sample, SampleKind, Signal, Terms, layered, merge},
};
use crate::{
    Result, RuntimeError,
    assets::AssetSource,
    model::{ModelMaterial, ModelTexture},
};
use ara_material::{Domain, Material};

pub(super) struct Prepared {
    pub material: ModelMaterial,
    pub textures: Vec<ModelTexture>,
    pub generations: Vec<TextureGeneration>,
    pub base_color_decode_srgb: bool,
    pub emissive_decode_srgb: bool,
    pub paint_layers: Vec<super::PaintLayer>,
}

/// Surface outputs in evaluation order, with their defaults.
const OUTPUTS: [(&str, &[f32]); 9] = [
    ("baseColor", &[1.; 3]),
    ("metallic", &[0.]),
    ("roughness", &[0.5]),
    ("normal", &[0., 0., 1.]),
    ("emissive", &[0.; 3]),
    ("opacityMask", &[1.]),
    ("transmission", &[0.]),
    ("clearCoat", &[0.]),
    ("clearCoatRoughness", &[0.1]),
];
/// The renderer blends at most four paint layers (`MaterialExtras::layer_materials`).
const MAX_PAINT_LAYERS: usize = 4;

pub(super) fn surface(definition: &Material, source: &dyn AssetSource) -> Result<Prepared> {
    if definition.domain != Domain::Surface {
        return Err(RuntimeError(format!(
            "{}: model material overrides require the surface domain",
            definition.id
        )));
    }
    let mut evaluate = Evaluator::new(definition, source);
    let mut outputs = Vec::new();
    for (name, default) in OUTPUTS {
        outputs.push(evaluate.output(name, default)?);
    }
    let tint = untint(&evaluate, &mut outputs[0])?;
    for (id, signal) in &outputs[1..] {
        if contains_tint(signal) {
            return Err(evaluate.bad(id, "vertex colour multiplies base colour only"));
        }
    }
    let mut layers = Vec::new();
    for (terms, signals) in split(&evaluate, outputs)? {
        let (mut material, base, emissive) = build(&evaluate, &definition.id, signals)?;
        material.vertex_color_tint = tint;
        layers.push(super::PaintLayer {
            terms,
            material,
            base_color_decode_srgb: base,
            emissive_decode_srgb: emissive,
        });
    }
    let first = &layers[0];
    let (mut material, base_color_decode_srgb, emissive_decode_srgb) = (
        first.material.clone(),
        first.base_color_decode_srgb,
        first.emissive_decode_srgb,
    );
    if layers.len() == 1 {
        layers.clear();
    } else {
        // The layered material's own record decides its pass: masked when any layer is.
        material.alpha_mode = layers
            .iter()
            .map(|l| l.material.alpha_mode)
            .max()
            .unwrap_or(0);
    }
    Ok(Prepared {
        material,
        textures: evaluate.textures,
        generations: evaluate.generations,
        base_color_decode_srgb,
        emissive_decode_srgb,
        paint_layers: layers,
    })
}

/// Strips the vertex-colour tint from base colour (whole, or from every paint layer's
/// leaf alike), reporting whether there was one.
fn untint(evaluate: &Evaluator<'_>, (id, signal): &mut (String, Signal)) -> Result<bool> {
    match std::mem::replace(signal, Signal::Constant(Vec::new())) {
        Signal::Tinted(inner) => {
            *signal = *inner;
            Ok(true)
        }
        Signal::Layered(terms) => {
            let tinted = terms
                .iter()
                .filter(|(_, s)| matches!(s, Signal::Tinted(_)))
                .count();
            if tinted != 0 && tinted != terms.len() {
                return Err(evaluate.bad(
                    id,
                    "multiply every paint layer's base colour by the vertex colour, or none",
                ));
            }
            *signal = Signal::Layered(
                terms
                    .into_iter()
                    .map(|(t, s)| match s {
                        Signal::Tinted(inner) => (t, *inner),
                        s => (t, s),
                    })
                    .collect(),
            );
            Ok(tinted != 0)
        }
        other => {
            *signal = other;
            Ok(false)
        }
    }
}

fn contains_tint(signal: &Signal) -> bool {
    match signal {
        Signal::Tinted(_) => true,
        Signal::Layered(terms) => terms.iter().any(|(_, s)| contains_tint(s)),
        _ => false,
    }
}

/// Output signals by node ID, in `OUTPUTS` order.
type Outputs = Vec<(String, Signal)>;

/// Splits evaluated outputs into paint layers: the common refinement of every
/// output's lerp tree (each layer's weight the product of the terms it lies under),
/// each with the output signals that layer sees. Unlayered outputs are shared.
fn split(
    evaluate: &Evaluator<'_>,
    outputs: Outputs,
) -> Result<Vec<(Terms, Outputs)>> {
    let mut weights: Vec<Terms> = vec![[0; 4]];
    for (_, signal) in &outputs {
        if let Signal::Layered(terms) = signal {
            let mut next = Vec::new();
            for known in &weights {
                for (terms, _) in terms {
                    if let Some(product) = merge(*known, *terms)
                        && !next.contains(&product)
                    {
                        next.push(product);
                    }
                }
            }
            weights = next;
        }
    }
    if weights.len() > MAX_PAINT_LAYERS {
        let id = outputs.first().map_or("", |(id, _)| id.as_str());
        return Err(evaluate.bad(
            id,
            &format!(
                "vertex-colour lerps form {} paint layers; the renderer blends at most {MAX_PAINT_LAYERS}",
                weights.len()
            ),
        ));
    }
    let mut layers = Vec::new();
    for weight in weights {
        let mut signals = Vec::new();
        for (id, signal) in &outputs {
            let leaf = layered(signal.clone())
                .into_iter()
                .find(|(terms, _)| merge(weight, *terms) == Some(weight))
                .map(|(_, leaf)| leaf)
                .ok_or_else(|| evaluate.bad(id, "paint layers do not cover every vertex colour"))?;
            signals.push((id.clone(), leaf));
        }
        layers.push((weight, signals));
    }
    Ok(layers)
}

/// One layer's PBR material from its output signals (`OUTPUTS` order), with whether
/// its base colour and emissive samples decode from sRGB.
fn build(
    evaluate: &Evaluator<'_>,
    name: &str,
    signals: Vec<(String, Signal)>,
) -> Result<(ModelMaterial, bool, bool)> {
    let [
        (base_id, base),
        (metal_id, metal),
        (rough_id, rough),
        (normal_id, normal),
        (emissive_id, emissive),
        (mask_id, mask),
        (transmission_id, transmission),
        (coat_id, coat),
        (coat_rough_id, coat_rough),
    ]: [(String, Signal); OUTPUTS.len()] = signals
        .try_into()
        .map_err(|_| RuntimeError(format!("{name}: material outputs are incomplete")))?;
    let mut material = empty(name);
    let (base, base_sample) = color(evaluate, &base_id, base)?;
    material.base_color[..3].copy_from_slice(&base);
    if let Some(sample) = &base_sample {
        material.base_color_texture = Some(sample.texture);
        material.base_color_uv = sample.uv;
    }
    let (metal, metal_sample) = channel(evaluate, &metal_id, metal, 2)?;
    let (rough, rough_sample) = channel(evaluate, &rough_id, rough, 1)?;
    material.metallic = metal;
    material.roughness = rough;
    let orm = match (&metal_sample, &rough_sample) {
        (Some(a), Some(b)) if a.texture == b.texture && a.uv == b.uv => Some(a),
        (Some(_), Some(_)) => {
            return Err(evaluate.bad(
                &rough_id,
                "metallic/roughness must share the same ORM texture and UVs",
            ));
        }
        (Some(sample), None) if rough == 0. => Some(sample),
        (None, Some(sample)) if metal == 0. => Some(sample),
        (None, None) => None,
        _ => {
            return Err(evaluate.bad(
                &rough_id,
                "an ORM sample needs both channels, or a zero factor for its unused channel",
            ));
        }
    };
    if let Some(sample) = orm {
        material.metallic_roughness_texture = Some(sample.texture);
        material.metallic_roughness_uv = sample.uv;
    }
    match normal {
        Signal::Constant(values) if values.as_slice() == [0., 0., 1.] => {}
        Signal::Sample(sample)
            if sample.kind == SampleKind::Normal && sample.normal_scale.is_finite() =>
        {
            material.normal_texture = Some(sample.texture);
            material.normal_uv = sample.uv;
            material.normal_scale = sample.normal_scale;
        }
        _ => {
            return Err(evaluate.bad(
                &normal_id,
                "normal must be tangent +Z or a normal texture sample with constant strength",
            ));
        }
    }
    let (emissive, emissive_sample) = color(evaluate, &emissive_id, emissive)?;
    material.emissive = emissive;
    if let Some(sample) = &emissive_sample {
        material.emissive_texture = Some(sample.texture);
        material.emissive_uv = sample.uv;
    }
    match mask {
        Signal::Constant(values) if values.len() == 1 && values[0].is_finite() => {
            if values[0] < 0.5 {
                material.alpha_mode = 1;
                material.base_color[3] = 0.;
            }
        }
        Signal::Sample(sample) if sample.kind == SampleKind::Data && sample.channels == [3]
            && sample.factors[0].is_finite() && material.base_color_texture == Some(sample.texture)
            && material.base_color_uv == sample.uv => {
                material.alpha_mode = 1;
                material.base_color[3] = sample.factors[0];
            }
        _=>return Err(evaluate.bad(&mask_id,"opacityMask must be constant or the alpha channel of the base-color texture with matching UVs")),
    }
    let (transmission, transmission_sample) = channel(evaluate, &transmission_id, transmission, 0)?;
    material.transmission_factor = transmission;
    if let Some(sample) = transmission_sample {
        material.transmission_texture = Some(sample.texture);
        material.transmission_uv = sample.uv;
    }
    if evaluate.scalar(&coat_id, coat)? != 0. {
        return Err(evaluate.bad(
            &coat_id,
            "clear coat is not represented by the current model PBR contract",
        ));
    }
    evaluate.scalar(&coat_rough_id, coat_rough)?;
    Ok((
        material,
        base_sample.is_some_and(|s| s.kind == SampleKind::Color),
        emissive_sample.is_some_and(|s| s.kind == SampleKind::Color),
    ))
}

fn color(evaluate: &Evaluator<'_>, id: &str, signal: Signal) -> Result<([f32; 3], Option<Sample>)> {
    match signal {
        Signal::Constant(values) if values.len() == 3 && values.iter().all(|v| v.is_finite()) => {
            Ok(([values[0], values[1], values[2]], None))
        }
        Signal::Sample(sample)
            if sample.kind != SampleKind::Normal
                && sample.channels == [0, 1, 2]
                && sample.factors.iter().all(|v| v.is_finite()) =>
        {
            Ok((
                [sample.factors[0], sample.factors[1], sample.factors[2]],
                Some(sample),
            ))
        }
        _ => Err(evaluate.bad(
            id,
            "color must be constant or one RGB texture sample with multiplicative factors",
        )),
    }
}

fn channel(
    evaluate: &Evaluator<'_>,
    id: &str,
    signal: Signal,
    channel: usize,
) -> Result<(f32, Option<Sample>)> {
    match signal {
        Signal::Constant(values) if values.len() == 1 && values[0].is_finite() => {
            Ok((values[0], None))
        }
        Signal::Sample(sample)
            if sample.kind == SampleKind::Data
                && sample.channels == [channel]
                && sample.factors[0].is_finite() =>
        {
            Ok((sample.factors[0], Some(sample)))
        }
        _ => Err(evaluate.bad(
            id,
            &format!("scalar must be constant or linear texture channel {channel}"),
        )),
    }
}

fn empty(name: &str) -> ModelMaterial {
    ModelMaterial {
        name: name.to_owned(),
        base_color: [1.; 4],
        metallic: 0.,
        roughness: 0.5,
        emissive: [0.; 3],
        base_color_texture: None,
        base_color_uv: Default::default(),
        normal_texture: None,
        normal_uv: Default::default(),
        normal_scale: 1.,
        metallic_roughness_texture: None,
        metallic_roughness_uv: Default::default(),
        occlusion_texture: None,
        occlusion_uv: Default::default(),
        occlusion_strength: 1.,
        emissive_texture: None,
        emissive_uv: Default::default(),
        transmission_factor: 0.,
        transmission_texture: None,
        transmission_uv: Default::default(),
        volume: Default::default(),
        double_sided: false,
        alpha_mode: 0,
        alpha_cutoff: 0.5,
        vertex_color_tint: false,
    }
}
