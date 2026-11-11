//! Ara's projected decals as Renderlab's box decals (`Engine::set_decals`). Both use
//! the unit cube around the entity and project along local -Z onto surfaces that face
//! +Z; Ara's V runs down local Y, so the box's Y axis is flipped on the way across.
//! Renderlab blends base colour, roughness, metalness and the normal; a decal's
//! emission, ambient occlusion and distance fade are reported and left out.
//!
//! Renderlab packs decal textures into one table as float texels, so each map is
//! reduced to at most `MAX_TEXTURE` texels a side. The table is rebuilt only when
//! the decals or their materials change.
use crate::gaps::Gaps;
use crate::rl::Engine;
use anyhow::Result;
use ara_runtime::{decal::DecalView, extract::RenderFrame, model::TextureTransform};
use renderlab::engine::{
    Decal, DecalBlend, DecalTexture, DecalTextureKind, DecalTextures, Mat4, Vec3,
};
use std::collections::HashMap;

/// Longest side of a decal map in Renderlab's table (16 bytes a texel, plus mips).
const MAX_TEXTURE: u32 = 512;
const FNV: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Tightly packed RGBA8 with its size.
struct Pixels {
    size: [u32; 2],
    rgba: Vec<u8>,
}

impl Pixels {
    fn decode(ktx2: &[u8]) -> Result<Self> {
        let decoded = ara_runtime_render::decode_texture(ktx2)?;
        let mut pixels = Self {
            size: decoded.size,
            rgba: decoded.rgba,
        };
        while pixels.size[0].max(pixels.size[1]) > MAX_TEXTURE {
            pixels = pixels.halved();
        }
        Ok(pixels)
    }

    /// The next smaller level, each texel the mean of the up to four it covers.
    fn halved(&self) -> Self {
        let [width, height] = self.size;
        let size = [(width / 2).max(1), (height / 2).max(1)];
        let mut rgba = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        for y in 0..size[1] {
            for x in 0..size[0] {
                for channel in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = ((x * 2 + dx).min(width - 1), (y * 2 + dy).min(height - 1));
                        sum += u32::from(self.rgba[((sy * width + sx) * 4 + channel) as usize]);
                    }
                    rgba.push(((sum + 2) / 4) as u8);
                }
            }
        }
        Self { size, rgba }
    }
}

/// Sets every texel's alpha to one: the map covers its whole box.
fn opaque(rgba: &mut [u8]) {
    for texel in rgba.as_chunks_mut::<4>().0 {
        texel[3] = 255;
    }
}

/// The decals last published and the textures they share.
#[derive(Default)]
pub(crate) struct Decals {
    /// Content key of the published set; `None` before the first publication.
    published: Option<u64>,
    /// Prepared maps by texture content, role and coverage source.
    textures: HashMap<u64, DecalTexture>,
}

/// Everything that decides how `view` is drawn.
fn key(hash: u64, view: &DecalView) -> u64 {
    let d = &view.decal;
    let mut hash = view
        .world
        .to_cols_array()
        .iter()
        .chain(d.color.0.iter())
        .chain(d.opacities().iter())
        .chain([d.fade_angle_start, d.fade_angle_end].iter())
        .fold(hash, |h, v| fnv(h, &v.to_bits().to_le_bytes()));
    hash = fnv(hash, &d.sort_order.to_le_bytes());
    match &view.material {
        Some(material) => fnv(hash, &material.digest.0),
        None => fnv(hash, b"pending"),
    }
}

impl Decals {
    /// Publishes the frame's decals when they differ from the published set.
    pub fn sync(
        &mut self,
        engine: &mut Engine,
        frame: &RenderFrame,
        gaps: &mut Gaps,
    ) -> Result<()> {
        let key = frame.decals.iter().fold(FNV, key);
        if self.published == Some(key) {
            return Ok(());
        }
        let mut used = HashMap::new();
        let mut decals = Vec::with_capacity(frame.decals.len());
        for view in &frame.decals {
            if let Some(decal) = self.decal(view, &mut used, gaps)? {
                decals.push(decal);
            }
        }
        if !decals.is_empty() && frame.draws.iter().any(|draw| !draw.receives_decals) {
            gaps.unsupported(
                "renderers that refuse decals (`receivesDecals` off)",
                "every opaque surface inside a decal's box receives it",
            );
        }
        engine.set_decals(&decals)?;
        // Maps no published decal uses any more are dropped.
        self.textures = used;
        self.published = Some(key);
        Ok(())
    }

    /// One decal; `None` while its material loads (Ara draws nothing either).
    fn decal(
        &mut self,
        view: &DecalView,
        used: &mut HashMap<u64, DecalTexture>,
        gaps: &mut Gaps,
    ) -> Result<Option<Decal>> {
        let Some(asset) = view.material.as_deref() else {
            return Ok(None);
        };
        let (d, m) = (&view.decal, &asset.material);
        let surface = asset.decal.as_ref();
        let opacities = d.opacities();
        if opacities[5] > 0. && (m.emissive != [0.; 3] || m.emissive_texture.is_some()) {
            gaps.unsupported("decal emission", "decals do not glow");
        }
        if opacities[6] > 0. && m.occlusion_texture.is_some() {
            gaps.unsupported(
                "decal ambient occlusion",
                "decals leave the surface's occlusion as it is",
            );
        }
        if d.fade_end_distance > 0. {
            gaps.unsupported(
                "decal distance fade",
                "decals stay fully visible at any distance",
            );
        }
        let moved = |uv: &TextureTransform| *uv != TextureTransform::default();
        if moved(&m.base_color_uv) || moved(&m.normal_uv) || moved(&m.metallic_roughness_uv) {
            gaps.unsupported(
                "texture transforms on decal maps",
                "each map fills the decal's box once",
            );
        }

        let mut texture = |slot: Option<usize>,
                           kind: DecalTextureKind,
                           coverage: Option<(usize, u32)>|
         -> Result<Option<DecalTexture>> {
            let bytes = |index: usize| asset.textures.get(index).map(|t| &*t.ktx2);
            let (base, cover) = (
                slot.and_then(bytes),
                coverage.and_then(|(i, c)| Some((bytes(i)?, c))),
            );
            if base.is_none() && cover.is_none() {
                return Ok(None);
            }
            let mut id = fnv(FNV, &[kind as u8]);
            id = base.map_or(fnv(id, b"white"), |b| fnv(id, b));
            id = cover.map_or(fnv(id, b"opaque"), |(b, c)| {
                fnv(fnv(id, b), &c.to_le_bytes())
            });
            let prepared = match self.textures.get(&id).or(used.get(&id)) {
                Some(prepared) => prepared.clone(),
                None => {
                    let cover = cover
                        .map(|(b, c)| Pixels::decode(b).map(|p| (p, c)))
                        .transpose()?;
                    let mut pixels = match base {
                        Some(base) => Pixels::decode(base)?,
                        // Coverage alone: a white map of the coverage map's size.
                        None => {
                            let size = cover.as_ref().map_or([1, 1], |(p, _)| p.size);
                            Pixels {
                                size,
                                rgba: vec![255; (size[0] * size[1] * 4) as usize],
                            }
                        }
                    };
                    if kind == DecalTextureKind::BaseColorSrgb {
                        // Renderlab reads coverage from this map's alpha.
                        match &cover {
                            Some((cover, channel)) if cover.size == pixels.size => {
                                let channel = (*channel).min(3) as usize;
                                for (texel, source) in pixels
                                    .rgba
                                    .as_chunks_mut::<4>()
                                    .0
                                    .iter_mut()
                                    .zip(cover.rgba.as_chunks::<4>().0)
                                {
                                    texel[3] = source[channel];
                                }
                            }
                            Some(_) => {
                                gaps.unsupported(
                                    "decal opacity maps sized differently from the base colour map",
                                    "the decal covers its whole box",
                                );
                                opaque(&mut pixels.rgba);
                            }
                            None => opaque(&mut pixels.rgba),
                        }
                    }
                    DecalTexture::from_rgba8(kind, pixels.size, &pixels.rgba)?
                }
            };
            used.insert(id, prepared.clone());
            Ok(Some(prepared))
        };
        let coverage = surface
            .and_then(|s| s.opacity_texture)
            .map(|(index, _, channel)| (index, channel));
        let textures = DecalTextures {
            base_color: texture(
                m.base_color_texture,
                DecalTextureKind::BaseColorSrgb,
                coverage,
            )?,
            metallic_roughness: texture(
                m.metallic_roughness_texture,
                DecalTextureKind::MetallicRoughness,
                None,
            )?,
            normal: texture(m.normal_texture, DecalTextureKind::Normal, None)?,
        };

        let unit = |v: f32| if v.is_finite() { v.clamp(0., 1.) } else { 0. };
        // Fade angles: Renderlab needs 0 <= start < end <= pi.
        let start = d.fade_angle_start.to_radians().clamp(0., 3.1);
        let end = d
            .fade_angle_end
            .to_radians()
            .clamp(start + 1e-3, std::f32::consts::PI);
        // Ara's V runs down local Y; Renderlab's runs up it.
        let transform = Mat4::from_cols_array(&view.world.to_cols_array())
            * Mat4::from_scale(Vec3::new(1., -1., 1.));
        if !transform.is_finite() || !transform.inverse().is_finite() {
            return Ok(None);
        }
        let tint = d.color.0;
        Ok(Some(Decal {
            transform,
            textures,
            base_color: [
                unit(m.base_color[0] * tint[0]),
                unit(m.base_color[1] * tint[1]),
                unit(m.base_color[2] * tint[2]),
                1.,
            ],
            roughness: unit(m.roughness),
            metallic: unit(m.metallic),
            // The component's colour alpha is already in `opacities`.
            opacity: unit(surface.map_or(1., |s| s.opacity)),
            channels: [opacities[0], opacities[2], opacities[3], opacities[1]].map(unit),
            blend: DecalBlend::Alpha,
            sort_order: d.sort_order,
            angle_fade_start: start,
            angle_fade_end: end,
            edge_fade: 0.,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halving_averages_blocks_and_reaches_the_cap() {
        // 4x2: left block all 100, right block all 200.
        let rgba = [100u8, 100, 200, 200, 100, 100, 200, 200]
            .iter()
            .flat_map(|v| [*v; 4])
            .collect();
        let half = Pixels { size: [4, 2], rgba }.halved();
        assert_eq!(half.size, [2, 1]);
        assert_eq!(half.rgba, [[100u8; 4], [200; 4]].concat());
    }

    #[test]
    fn a_decal_key_follows_its_pose_and_opacities() {
        let view = |x: f32, opacity: f32| DecalView {
            entity: 1,
            address: "e_1".into(),
            label: "decal".into(),
            world: glam34::Mat4::from_translation(glam34::Vec3::new(x, 0., 0.)),
            decal: ara_runtime::decal::Decal {
                normal_opacity: opacity,
                ..Default::default()
            },
            material: None,
        };
        let base = key(FNV, &view(0., 1.));
        assert_eq!(base, key(FNV, &view(0., 1.)));
        assert_ne!(base, key(FNV, &view(1., 1.)));
        assert_ne!(base, key(FNV, &view(0., 0.5)));
    }
}
