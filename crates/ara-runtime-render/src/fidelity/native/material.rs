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

pub(super) fn apply_draw(out: &mut Material, draw: &Draw, source: Option<Source<'_>>) {
    if let Some(source) = source {
        let m = source.material();
        out.pbr.base_color = std::array::from_fn(|i| m.base_color[i] * draw.color[i]);
        let emission = draw.emissive_factor.unwrap_or(m.emissive);
        out.pbr.emissive_roughness = [
            emission[0] * draw.emissive[0],
            emission[1] * draw.emissive[1],
            emission[2] * draw.emissive[2],
            m.roughness,
        ];
        out.pbr.surface = [m.metallic, m.normal_scale, 1.0, m.alpha_cutoff];
        out.pbr.flags[0] = m.alpha_mode;
    } else {
        out.pbr.base_color = draw.color;
        let emission = if matches!(draw.geometry, ara_runtime::extract::Geometry::Model { .. }) {
            let source = draw.emissive_factor.unwrap_or([0.0; 3]);
            std::array::from_fn(|i| source[i] * draw.emissive[i])
        } else {
            draw.emissive
        };
        out.pbr.emissive_roughness = [emission[0], emission[1], emission[2], draw.roughness];
        out.pbr.surface = [draw.metallic, 1.0, 1.0, 0.5];
        out.pbr.flags[0] = u32::from(draw.color[3] < 1.0) * 2;
    }
}

pub(super) fn resolve_known(
    out: &mut Material,
    source: Source<'_>,
    keys: &BTreeMap<(String, usize), u32>,
) -> bool {
    properties(out, source);
    let m = source.material();
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
            let Some(&known) = keys.get(&source.key(index)) else {
                return false;
            };
            *id = known;
        }
    }
    assign(out, ids);
    true
}

fn assign(out: &mut Material, ids: [u32; 6]) {
    out.pbr.textures.copy_from_slice(&ids[..4]);
    out.extras.occlusion_texture = ids[4];
    out.extras.transmission_texture = ids[5];
}

fn properties(out: &mut Material, source: Source<'_>) {
    let m = source.material();
    out.pbr.base_color = m.base_color;
    out.pbr.emissive_roughness = [m.emissive[0], m.emissive[1], m.emissive[2], m.roughness];
    out.pbr.surface = [m.metallic, m.normal_scale, 1.0, m.alpha_cutoff];
    out.pbr.flags[0] = m.alpha_mode;
    out.pbr.flags[1] = (u32::from(source.decode(0)) * BASE_COLOR_DECODE_SRGB)
        | (u32::from(source.decode(3)) * EMISSIVE_DECODE_SRGB);
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
    out.extras.transmission_factor = transmission(source);
    out.extras.volume = render::Volume {
        ior: m.volume.ior,
        thickness: m.volume.thickness,
        attenuation_color: m.volume.attenuation_color,
        attenuation_distance: m.volume.attenuation_distance,
    }
    .pack();
    out.extras.reconstruct_normal_basis = u32::from(source.raw());
}

/// The transmission factor the renderer sees. KHR_materials_transmission is the
/// dielectric layer's (shaders multiply it by 1 - metal per texel), and any
/// transmission routes a material to the refractive forward pass: a mostly-metal
/// surface (the mean of metallic factor times its map at least one half, e.g.
/// brass authored with transmission) keeps none, so it stays in the G-buffer with
/// its reflections, AO and probe GI.
fn transmission(source: Source<'_>) -> f32 {
    let m = source.material();
    if m.transmission_factor <= 0. {
        return 0.;
    }
    let map = m.metallic_roughness_texture.map_or(1., |t| {
        crate::mean_texel(&source.texture(t).ktx2).map_or(1., |mean| mean[2])
    });
    if m.metallic * map >= 0.5 {
        0.
    } else {
        m.transmission_factor
    }
}

impl Builder {
    pub(super) fn material(&mut self, draw: &Draw) -> Result<usize> {
        let mut material = Material::default();
        if let Some(source) = Source::of(draw) {
            self.source_material(&mut material, source)?;
        }
        apply_draw(&mut material, draw, Source::of(draw));
        Ok(self.add_material(material))
    }

    pub(super) fn source_material(&mut self, out: &mut Material, source: Source<'_>) -> Result<()> {
        properties(out, source);
        let m = source.material();
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
        assign(out, ids);
        Ok(())
    }
}
