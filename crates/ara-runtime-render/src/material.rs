//! Material source and view identity shared by Basic's residency and draw lowering.
use ara_runtime::{
    extract::{Draw, Geometry},
    material::MaterialAsset,
    model::{Model, ModelMaterial, ModelTexture},
};

#[derive(Clone, Copy)]
pub(crate) enum Source<'a> {
    Model(&'a Model, &'a ModelMaterial),
    Native(&'a MaterialAsset),
}
impl<'a> Source<'a> {
    pub fn of(draw: &'a Draw) -> Option<Self> {
        if let Some(native) = draw.material.as_deref() {
            return Some(Self::Native(native));
        }
        if let Geometry::Model { model, primitive } = &draw.geometry {
            return model.primitives[*primitive]
                .material
                .and_then(|slot| model.materials.get(slot))
                .map(|material| Self::Model(model, material));
        }
        None
    }
    pub fn material(self) -> &'a ModelMaterial {
        match self {
            Self::Model(_, m) => m,
            Self::Native(m) => &m.material,
        }
    }
    pub fn texture(self, index: usize) -> &'a ModelTexture {
        match self {
            Self::Model(m, _) => &m.textures[index],
            Self::Native(m) => &m.textures[index],
        }
    }
    pub fn key(self, index: usize) -> (String, usize) {
        match self {
            Self::Model(m, _) => (m.digest.to_string(), index),
            Self::Native(m) => {
                let generation = &m.texture_generations[index];
                // All native graph views are raw, including data use of a color import.
                (
                    format!("native-unorm:{}:{}", generation.asset, generation.digest),
                    0,
                )
            }
        }
    }
    pub fn decode(self, slot: usize) -> bool {
        match self {
            Self::Native(m) => match slot {
                0 => m.base_color_decode_srgb,
                3 => m.emissive_decode_srgb,
                _ => false,
            },
            _ => false,
        }
    }
    pub fn raw(self) -> bool {
        matches!(self, Self::Native(_))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ara_asset::Digest;
    use ara_asset_format::{TextureFormat, write_ktx2};
    use ara_runtime::{
        components::MeshShape, extract::RenderFrame, material::TextureGeneration,
        model::TextureUsage,
    };
    use std::sync::Arc;

    #[test]
    fn native_views_share_texture_generations_and_preserve_decode_policy_and_factors() {
        let texture = ModelTexture {
            name: "texture.ktx2".into(),
            usage: TextureUsage::Color,
            ktx2: write_ktx2(4, 4, TextureFormat::Bc7Srgb, &[vec![0; 16]])
                .unwrap()
                .into(),
        };
        let asset = |digest: &[u8]| {
            Arc::new(MaterialAsset {
                asset: "gr_test".into(),
                digest: Digest::of(digest),
                material: ModelMaterial {
                    base_color: [0.25, 0.5, 0.75, 0.8],
                    base_color_texture: Some(0),
                    emissive_texture: Some(0),
                    alpha_mode: 1,
                    ..Default::default()
                },
                textures: vec![texture.clone()],
                texture_generations: vec![TextureGeneration {
                    asset: "as_shared".into(),
                    digest: Digest::of(b"image"),
                }],
                base_color_decode_srgb: true,
                emissive_decode_srgb: true,
            })
        };
        let draw = |entity, material| Draw {
            entity,
            address: format!("e_{entity}"),
            label: "native".into(),
            geometry: Geometry::Shape {
                shape: MeshShape::Box,
                size: [1.; 3],
            },
            world: glam::Mat4::IDENTITY,
            color: [0.5; 4],
            roughness: 1.,
            metallic: 0.,
            emissive: [1.; 3],
            emissive_factor: None,
            material: Some(material),
            skin: None,
        };
        let first = draw(1, asset(b"first"));
        let second = draw(2, asset(b"second"));
        assert_ne!(crate::material_key(&first), crate::material_key(&second));
        let mut frame = RenderFrame::default();
        frame.draws.extend([first, second]);
        let mut presenter =
            crate::Presenter::new([16, 16], vec![], "target/unused-native-material-cache");
        let scene = presenter.scene(&frame, &[]).unwrap();
        assert_eq!(
            scene.textures.len(),
            3,
            "two defaults and one shared raw texture"
        );
        for material in &scene.materials {
            assert_eq!(material.flags[0], 1);
            assert_eq!(
                material.flags[1],
                ara_render::scene::BASE_COLOR_DECODE_SRGB | ara_render::scene::EMISSIVE_DECODE_SRGB
            );
            assert_eq!(material.flags[3], 0);
            assert_eq!(material.base_color, [0.125, 0.25, 0.375, 0.4]);
            assert!(!scene.textures[material.textures[0] as usize].srgb);
        }
        assert_eq!(scene.materials[0].textures, scene.materials[1].textures);
    }
}
