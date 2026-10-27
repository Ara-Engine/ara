//! Renderer-neutral PBR data, with an independent coordinate transform per slot.
use super::{TextureTransform, TextureUsage, floats, uv};
use crate::Result;
use serde_json::Value;
use std::collections::BTreeMap;

/// Metallic-roughness factors and slots into [`crate::model::Model::textures`].
#[derive(Debug, Clone, PartialEq)]
pub struct ModelMaterial {
    /// Source material name, retained for authoring selectors.
    pub name: String,
    /// Linear base color factor.
    pub base_color: [f32; 4],
    /// Metalness factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
    /// Emissive factor, including KHR_materials_emissive_strength exactly once.
    pub emissive: [f32; 3],
    /// Base color texture index.
    pub base_color_texture: Option<usize>,
    /// Base color coordinates and transform.
    pub base_color_uv: TextureTransform,
    /// Tangent-space normal map index (BC5 XY).
    pub normal_texture: Option<usize>,
    /// Normal map coordinates and transform, independently of color coordinates.
    pub normal_uv: TextureTransform,
    /// Normal map strength (`normalTexture.scale`).
    pub normal_scale: f32,
    /// Metallic (B) / roughness (G) texture index.
    pub metallic_roughness_texture: Option<usize>,
    /// Metallic/roughness coordinates and transform.
    pub metallic_roughness_uv: TextureTransform,
    /// Occlusion (R) texture index.
    pub occlusion_texture: Option<usize>,
    /// Occlusion coordinates and transform.
    pub occlusion_uv: TextureTransform,
    /// Interpolation from unoccluded lighting to the sampled occlusion value.
    pub occlusion_strength: f32,
    /// Emissive color texture index.
    pub emissive_texture: Option<usize>,
    /// Emissive coordinates and transform.
    pub emissive_uv: TextureTransform,
    /// KHR_materials_transmission fraction; independent of alpha coverage.
    pub transmission_factor: f32,
    /// Linear transmission (R) texture index.
    pub transmission_texture: Option<usize>,
    /// Transmission coordinates and transform.
    pub transmission_uv: TextureTransform,
    /// KHR_materials_ior and KHR_materials_volume: how transmitted light refracts
    /// and is absorbed.
    pub volume: ModelVolume,
    /// Alpha mode: 0 opaque, 1 mask, 2 blend.
    pub alpha_mode: u32,
    /// Mask cutoff.
    pub alpha_cutoff: f32,
}

/// A transmissive material's medium (KHR_materials_ior, KHR_materials_volume).
/// The volume's thickness texture is not represented: the factor applies alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelVolume {
    /// Index of refraction (glTF default 1.5).
    pub ior: f32,
    /// Thickness beneath the surface in mesh units; zero is thin-walled.
    pub thickness: f32,
    /// Linear colour white light keeps after `attenuation_distance` in the medium.
    pub attenuation_color: [f32; 3],
    /// World distance (m) for `attenuation_color`; infinite: no absorption.
    pub attenuation_distance: f32,
}

impl Default for ModelVolume {
    fn default() -> Self {
        Self {
            ior: 1.5,
            thickness: 0.,
            attenuation_color: [1.; 3],
            attenuation_distance: f32::INFINITY,
        }
    }
}

pub(super) fn decode(
    m: &Value,
    textures: &[Value],
    by_image: &BTreeMap<(usize, TextureUsage), usize>,
) -> Result<ModelMaterial> {
    let pbr = &m["pbrMetallicRoughness"];
    let transmission = &m["extensions"]["KHR_materials_transmission"];
    let volume = &m["extensions"]["KHR_materials_volume"];
    let strength = m["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"]
        .as_f64()
        .unwrap_or(1.) as f32;
    let slot = |info: &Value, usage: TextureUsage| {
        info["index"]
            .as_u64()
            .and_then(|t| textures.get(t as usize))
            .and_then(|t| t["source"].as_u64())
            .and_then(|image| by_image.get(&(image as usize, usage)).copied())
    };
    Ok(ModelMaterial {
        name: m["name"].as_str().unwrap_or_default().to_owned(),
        base_color: floats(&pbr["baseColorFactor"], [1.; 4]),
        metallic: pbr["metallicFactor"].as_f64().unwrap_or(1.) as f32,
        roughness: pbr["roughnessFactor"].as_f64().unwrap_or(1.) as f32,
        emissive: floats(&m["emissiveFactor"], [0.; 3]).map(|v| v * strength),
        base_color_texture: slot(&pbr["baseColorTexture"], TextureUsage::Color),
        base_color_uv: uv::transform(&pbr["baseColorTexture"])?,
        normal_texture: slot(&m["normalTexture"], TextureUsage::Normal),
        normal_uv: uv::transform(&m["normalTexture"])?,
        normal_scale: m["normalTexture"]["scale"].as_f64().unwrap_or(1.) as f32,
        metallic_roughness_texture: slot(&pbr["metallicRoughnessTexture"], TextureUsage::Linear),
        metallic_roughness_uv: uv::transform(&pbr["metallicRoughnessTexture"])?,
        occlusion_texture: slot(&m["occlusionTexture"], TextureUsage::Linear),
        occlusion_uv: uv::transform(&m["occlusionTexture"])?,
        occlusion_strength: m["occlusionTexture"]["strength"].as_f64().unwrap_or(1.) as f32,
        emissive_texture: slot(&m["emissiveTexture"], TextureUsage::Color),
        emissive_uv: uv::transform(&m["emissiveTexture"])?,
        transmission_factor: transmission["transmissionFactor"].as_f64().unwrap_or(0.) as f32,
        transmission_texture: slot(&transmission["transmissionTexture"], TextureUsage::Linear),
        transmission_uv: uv::transform(&transmission["transmissionTexture"])?,
        volume: ModelVolume {
            ior: m["extensions"]["KHR_materials_ior"]["ior"]
                .as_f64()
                .unwrap_or(1.5) as f32,
            thickness: volume["thicknessFactor"].as_f64().unwrap_or(0.) as f32,
            attenuation_color: floats(&volume["attenuationColor"], [1.; 3]),
            attenuation_distance: volume["attenuationDistance"]
                .as_f64()
                .map_or(f32::INFINITY, |d| d as f32),
        },
        alpha_mode: match m["alphaMode"].as_str() {
            Some("MASK") => 1,
            Some("BLEND") => 2,
            _ => 0,
        },
        alpha_cutoff: m["alphaCutoff"].as_f64().unwrap_or(0.5) as f32,
    })
}

impl Default for ModelMaterial {
    fn default() -> Self {
        decode(&Value::Null, &[], &BTreeMap::new())
            .expect("default glTF factors and identity transforms are valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ior_and_volume_extensions_decode_with_gltf_defaults() {
        let glass = serde_json::json!({"extensions": {
            "KHR_materials_transmission": {"transmissionFactor": 1.0},
            "KHR_materials_ior": {"ior": 1.45},
            "KHR_materials_volume": {
                "thicknessFactor": 0.6,
                "attenuationColor": [0.2, 0.55, 0.45],
                "attenuationDistance": 1.0
            }
        }});
        let m = decode(&glass, &[], &BTreeMap::new()).expect("glass");
        assert_eq!(m.transmission_factor, 1.);
        assert_eq!(
            m.volume,
            ModelVolume {
                ior: 1.45,
                thickness: 0.6,
                attenuation_color: [0.2, 0.55, 0.45],
                attenuation_distance: 1.,
            }
        );
        assert_eq!(ModelMaterial::default().volume, ModelVolume::default());
    }
}
