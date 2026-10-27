use super::{
    TextureGeneration,
    expr::{Evaluator, Sample, SampleKind, Signal},
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
}

pub(super) fn surface(definition: &Material, source: &dyn AssetSource) -> Result<Prepared> {
    if definition.domain != Domain::Surface {
        return Err(RuntimeError(format!(
            "{}: model material overrides require the surface domain",
            definition.id
        )));
    }
    let mut evaluate = Evaluator::new(definition, source);
    let mut material = empty(&definition.id);
    let (base_id, base) = evaluate.output("baseColor", &[1.; 3])?;
    let (base, base_sample) = color(&evaluate, &base_id, base)?;
    material.base_color[..3].copy_from_slice(&base);
    if let Some(sample) = &base_sample {
        material.base_color_texture = Some(sample.texture);
        material.base_color_uv = sample.uv;
    }
    let (metal_id, metal) = evaluate.output("metallic", &[0.])?;
    let (metal, metal_sample) = channel(&evaluate, &metal_id, metal, 2)?;
    let (rough_id, rough) = evaluate.output("roughness", &[0.5])?;
    let (rough, rough_sample) = channel(&evaluate, &rough_id, rough, 1)?;
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
    let (normal_id, normal) = evaluate.output("normal", &[0., 0., 1.])?;
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
    let (emissive_id, emissive) = evaluate.output("emissive", &[0.; 3])?;
    let (emissive, emissive_sample) = color(&evaluate, &emissive_id, emissive)?;
    material.emissive = emissive;
    if let Some(sample) = &emissive_sample {
        material.emissive_texture = Some(sample.texture);
        material.emissive_uv = sample.uv;
    }
    let (mask_id, mask) = evaluate.output("opacityMask", &[1.])?;
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
    let (transmission_id, transmission) = evaluate.output("transmission", &[0.])?;
    let (transmission, transmission_sample) =
        channel(&evaluate, &transmission_id, transmission, 0)?;
    material.transmission_factor = transmission;
    if let Some(sample) = transmission_sample {
        material.transmission_texture = Some(sample.texture);
        material.transmission_uv = sample.uv;
    }
    let (coat_id, coat) = evaluate.output("clearCoat", &[0.])?;
    if evaluate.scalar(&coat_id, coat)? != 0. {
        return Err(evaluate.bad(
            &coat_id,
            "clear coat is not represented by the current model PBR contract",
        ));
    }
    let (coat_rough_id, coat_rough) = evaluate.output("clearCoatRoughness", &[0.1])?;
    evaluate.scalar(&coat_rough_id, coat_rough)?;
    Ok(Prepared {
        material,
        textures: evaluate.textures,
        generations: evaluate.generations,
        base_color_decode_srgb: base_sample.is_some_and(|s| s.kind == SampleKind::Color),
        emissive_decode_srgb: emissive_sample.is_some_and(|s| s.kind == SampleKind::Color),
    })
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
        alpha_mode: 0,
        alpha_cutoff: 0.5,
    }
}
