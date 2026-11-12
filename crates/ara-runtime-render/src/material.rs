//! Material source and view identity shared by Basic's residency and draw lowering.
use ara_runtime::{
    extract::{Draw, Geometry},
    material::{MaterialAsset, PaintLayer},
    model::{Model, ModelMaterial, ModelTexture},
};

#[derive(Clone, Copy)]
pub(crate) enum Source<'a> {
    Model(&'a Model, &'a ModelMaterial),
    Native(&'a MaterialAsset),
    /// One vertex-paint layer of a native material: its own factors and slots over
    /// the asset's textures.
    Layer(&'a MaterialAsset, &'a PaintLayer),
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
            Self::Layer(_, layer) => &layer.material,
        }
    }
    pub fn texture(self, index: usize) -> &'a ModelTexture {
        match self {
            Self::Model(m, _) => &m.textures[index],
            Self::Native(m) | Self::Layer(m, _) => &m.textures[index],
        }
    }
    pub fn key(self, index: usize) -> (String, usize) {
        match self {
            Self::Model(m, _) => (m.digest.to_string(), index),
            Self::Native(m) | Self::Layer(m, _) => {
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
            Self::Layer(_, layer) => match slot {
                0 => layer.base_color_decode_srgb,
                3 => layer.emissive_decode_srgb,
                _ => false,
            },
            _ => false,
        }
    }
    pub fn raw(self) -> bool {
        matches!(self, Self::Native(_) | Self::Layer(..))
    }
}
