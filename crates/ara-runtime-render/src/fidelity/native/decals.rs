//! Deferred decals: each `Decal` component with a loaded decal-domain material becomes
//! a native decal and one material row (its maps registered like any material's).
use super::*;
use crate::material::Source;
use ara_runtime::{decal::DecalView, material::PaintLayer};

impl Builder {
    pub(super) fn decals(&mut self, decals: &[DecalView]) -> Result<()> {
        for view in decals {
            if let Some(decal) = decal(self, view)? {
                self.scene.decals.push(decal);
            }
        }
        Ok(())
    }
}

/// The native decal of `view`, registering its material row; `None` while its
/// material is missing or not a decal material (nothing draws).
fn decal(builder: &mut Builder, view: &DecalView) -> Result<Option<render::Decal>> {
    let Some(asset) = view.material.as_deref() else {
        return Ok(None);
    };
    let Some(surface) = &asset.decal else {
        return Ok(None);
    };
    // The opacity map rides in the transmission slot (5), which decals do not use.
    let mut layer = PaintLayer {
        terms: [0; 4],
        material: asset.material.clone(),
        base_color_decode_srgb: asset.base_color_decode_srgb,
        emissive_decode_srgb: asset.emissive_decode_srgb,
    };
    layer.material.transmission_texture = surface.opacity_texture.map(|(texture, ..)| texture);
    if let Some((_, uv, _)) = surface.opacity_texture {
        layer.material.transmission_uv = uv;
    }
    let mut material = Material::default();
    builder.source_material(&mut material, Source::Layer(asset, &layer))?;
    let tint = view.decal.color.0;
    let base = asset.material.base_color;
    material.pbr.base_color = [
        (base[0] * tint[0]).clamp(0.0, 1.0),
        (base[1] * tint[1]).clamp(0.0, 1.0),
        (base[2] * tint[2]).clamp(0.0, 1.0),
        surface.opacity.clamp(0.0, 1.0),
    ];
    let row = builder.add_material(material);
    let d = &view.decal;
    Ok(Some(render::Decal {
        entity: view.entity,
        name: view.label.to_owned(),
        world: glam30::Mat4::from_cols_array(&view.world.to_cols_array()),
        material: row,
        opacity: d.opacities(),
        opacity_channel: surface.opacity_texture.map_or(4, |(.., channel)| channel.min(4)),
        occlusion: [
            surface.occlusion,
            if asset.material.occlusion_texture.is_some() {
                surface.occlusion_channel.min(4) as f32
            } else {
                4.0
            },
        ],
        specular: surface.specular,
        fade_distance: [d.fade_start_distance, d.fade_end_distance],
        fade_angle: [d.fade_angle_start, d.fade_angle_end],
        sort_order: d.sort_order,
        fade_screen_size: d.fade_screen_size,
    }))
}

/// A decal's live parameters for `update_frame` (its material row stays).
pub(super) fn update(decal: &mut render::Decal, view: &DecalView) {
    let d = &view.decal;
    decal.world = glam30::Mat4::from_cols_array(&view.world.to_cols_array());
    decal.opacity = d.opacities();
    decal.fade_distance = [d.fade_start_distance, d.fade_end_distance];
    decal.fade_angle = [d.fade_angle_start, d.fade_angle_end];
    decal.sort_order = d.sort_order;
    decal.fade_screen_size = d.fade_screen_size;
}

/// What a live frame must match to stream decal edits: identity, material
/// generation and the colour (folded into the material row).
pub(super) fn identity(view: &DecalView) -> (u64, Option<[u8; 32]>, [u32; 3]) {
    (
        view.entity,
        view.material
            .as_ref()
            .filter(|m| m.decal.is_some())
            .map(|m| m.digest.0),
        [0, 1, 2].map(|i| view.decal.color.0[i].to_bits()),
    )
}
