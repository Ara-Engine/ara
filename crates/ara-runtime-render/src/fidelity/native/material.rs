use super::*;
use crate::material::Source;
use ara_render::{
    instanced::NO_TEXTURE,
    scene::{BASE_COLOR_DECODE_SRGB, EMISSIVE_DECODE_SRGB},
};
use ara_runtime::model::TextureTransform;
use ara_texture_stream::{NativeSource, NativeView};

fn uv(source: TextureTransform) -> render::TextureTransform {
    render::TextureTransform {
        offset: source.offset,
        scale: source.scale,
        rotation: source.rotation,
        tex_coord: source.tex_coord,
        reserved: [0; 2],
    }
}

impl Builder {
    pub(super) fn material(&mut self, draw: &Draw) -> Result<usize> {
        let mut material = Material::default();
        material.pbr.base_color = draw.color;
        material.pbr.emissive_roughness = [
            draw.emissive[0],
            draw.emissive[1],
            draw.emissive[2],
            draw.roughness,
        ];
        material.pbr.surface = [draw.metallic, 1.0, 1.0, 0.5];
        material.pbr.flags[0] = u32::from(draw.color[3] < 1.0) * 2;
        if let Some(source) = Source::of(draw) {
            self.source_material(&mut material, source)?;
            for (factor, multiplier) in material.pbr.base_color.iter_mut().zip(draw.color) {
                *factor *= multiplier;
            }
            let emission = draw.emissive_factor.unwrap_or(source.material().emissive);
            for (value, (factor, multiplier)) in material.pbr.emissive_roughness[..3]
                .iter_mut()
                .zip(emission.into_iter().zip(draw.emissive))
            {
                *value = factor * multiplier;
            }
        } else if matches!(draw.geometry, ara_runtime::extract::Geometry::Model { .. }) {
            let emission = draw.emissive_factor.unwrap_or([0.0; 3]);
            for (value, (factor, multiplier)) in material.pbr.emissive_roughness[..3]
                .iter_mut()
                .zip(emission.into_iter().zip(draw.emissive))
            {
                *value = factor * multiplier;
            }
        }
        Ok(self.add_material(material))
    }

    pub(super) fn source_material(&mut self, out: &mut Material, source: Source<'_>) -> Result<()> {
        let m = source.material();
        out.pbr.base_color = m.base_color;
        out.pbr.emissive_roughness = [m.emissive[0], m.emissive[1], m.emissive[2], m.roughness];
        out.pbr.surface = [m.metallic, m.normal_scale, 1.0, m.alpha_cutoff];
        out.pbr.flags[0] = m.alpha_mode;
        out.pbr.flags[1] = (u32::from(source.decode(0)) * BASE_COLOR_DECODE_SRGB)
            | (u32::from(source.decode(3)) * EMISSIVE_DECODE_SRGB);
        let slots = [
            m.base_color_texture,
            m.normal_texture,
            m.metallic_roughness_texture,
            m.emissive_texture,
            m.occlusion_texture,
            m.transmission_texture,
        ];
        let mut ids = [NO_TEXTURE; 6];
        for (id, slot) in ids.iter_mut().zip(slots) {
            if let Some(index) = slot {
                let key = source.key(index);
                *id = if let Some(&existing) = self.texture_keys.get(&key) {
                    existing
                } else {
                    let texture = source.texture(index);
                    let hash: [u8; 32] = Sha256::digest(&texture.ktx2).into();
                    let native = Arc::new(NativeSource::from_cooked(Arc::clone(&texture.ktx2))?);
                    let id = self
                        .textures
                        .register_view(
                            hash,
                            native,
                            if source.raw() {
                                NativeView::Raw
                            } else {
                                NativeView::Cooked
                            },
                        )?
                        .index();
                    self.texture_keys.insert(key, id);
                    self.scene.textures = self.scene.textures.max(id + 1);
                    id
                };
            }
        }
        out.pbr.textures.copy_from_slice(&ids[..4]);
        out.extras.occlusion_texture = ids[4];
        out.extras.transmission_texture = ids[5];
        out.extras.uv = [
            m.base_color_uv,
            m.normal_uv,
            m.metallic_roughness_uv,
            m.emissive_uv,
            m.occlusion_uv,
            m.transmission_uv,
        ]
        .map(uv);
        out.extras.occlusion_strength = m.occlusion_strength;
        out.extras.transmission_factor = m.transmission_factor;
        out.extras.reconstruct_normal_basis = u32::from(source.raw());
        Ok(())
    }
}
