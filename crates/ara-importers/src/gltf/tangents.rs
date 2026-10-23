//! Match missing-tangent generation to the normal texture's final coordinates.
use crate::{Vertex, invalid};
use ara_asset::Result;

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(default)]
pub(super) struct NormalUv {
    #[serde(rename = "texCoord")]
    set: u32,
    offset: [f32; 2],
    scale: [f32; 2],
    rotation: f32,
}

impl Default for NormalUv {
    fn default() -> Self {
        Self {
            set: 0,
            offset: [0.; 2],
            scale: [1.; 2],
            rotation: 0.,
        }
    }
}

impl NormalUv {
    pub fn for_material(material: &gltf::Material<'_>) -> Result<Self> {
        let normal = material.normal_texture();
        let extension = normal
            .as_ref()
            .and_then(|n| n.extension_value("KHR_texture_transform"));
        let mut result = extension
            .map(|v| serde_json::from_value::<Self>(v.clone()))
            .transpose()
            .map_err(invalid)?
            .unwrap_or_default();
        if extension.is_none_or(|e| e.get("texCoord").is_none()) {
            result.set = normal.as_ref().map_or(0, |n| n.tex_coord());
        }
        if result.set > 1 {
            return Err(invalid(
                "normal texture selects an unsupported UV set; cooked meshes retain TEXCOORD_0 and TEXCOORD_1",
            ));
        }
        if !result
            .offset
            .iter()
            .chain(&result.scale)
            .chain(std::iter::once(&result.rotation))
            .all(|v| v.is_finite())
        {
            return Err(invalid("non-finite normal texture transform"));
        }
        Ok(result)
    }

    pub fn apply(&self, vertex: &Vertex) -> [f32; 2] {
        let uv = if self.set == 1 {
            vertex.uv1
        } else {
            vertex.uv0
        };
        let [x, y] = std::array::from_fn(|i| uv[i] * self.scale[i]);
        let (sin, cos) = self.rotation.sin_cos();
        [
            self.offset[0] + cos * x - sin * y,
            self.offset[1] + sin * x + cos * y,
        ]
    }
}
