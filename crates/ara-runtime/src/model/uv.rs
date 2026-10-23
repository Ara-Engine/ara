//! Per-texture glTF coordinates; transforms cannot be baked into one shared UV set.
use crate::{Result, RuntimeError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A texture slot's `KHR_texture_transform`, applied as offset + rotation * scale * UV.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TextureTransform {
    /// Translation in texture-coordinate units.
    pub offset: [f32; 2],
    /// Independent U/V scale before rotation.
    pub scale: [f32; 2],
    /// Counter-clockwise rotation in radians.
    pub rotation: f32,
    /// Source texture-coordinate set; the extension overrides the slot's texCoord.
    #[serde(deserialize_with = "coordinate")]
    pub tex_coord: u32,
}

// gltf-json serializes an absent optional extension selector as null in scene metadata.
fn coordinate<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<u32, D::Error> {
    Option::<u32>::deserialize(deserializer).map(|value| value.unwrap_or(0))
}

impl Default for TextureTransform {
    fn default() -> Self {
        Self {
            offset: [0.; 2],
            scale: [1.; 2],
            rotation: 0.,
            tex_coord: 0,
        }
    }
}

impl TextureTransform {
    /// Applies the transform to a coordinate from this slot's selected UV set.
    pub fn apply(&self, uv: [f32; 2]) -> [f32; 2] {
        let [x, y] = std::array::from_fn(|i| uv[i] * self.scale[i]);
        let (sin, cos) = self.rotation.sin_cos();
        [
            self.offset[0] + cos * x - sin * y,
            self.offset[1] + sin * x + cos * y,
        ]
    }
}

pub(super) fn transform(info: &Value) -> Result<TextureTransform> {
    let extension = info
        .get("extensions")
        .and_then(|v| v.get("KHR_texture_transform"));
    let mut value = extension
        .map(|v| serde_json::from_value::<TextureTransform>(v.clone()))
        .transpose()
        .map_err(|e| RuntimeError(format!("invalid texture transform: {e}")))?
        .unwrap_or_default();
    if extension.is_none_or(|v| v.get("texCoord").is_none_or(Value::is_null)) {
        value.tex_coord = info
            .get("texCoord")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| RuntimeError(format!("invalid texture coordinate index: {e}")))?
            .unwrap_or(0);
    }
    if !value
        .offset
        .iter()
        .chain(&value.scale)
        .chain(std::iter::once(&value.rotation))
        .all(|x| x.is_finite())
    {
        return Err(RuntimeError("non-finite texture transform".into()));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn texture_transform_scales_then_rotates_then_offsets_and_overrides_uv_selection() {
        let info = json!({"texCoord":0,"extensions":{"KHR_texture_transform":{
            "offset":[0.1,0.2],"scale":[2,3],"rotation":std::f32::consts::FRAC_PI_2,"texCoord":1
        }}});
        let transform = transform(&info).expect("valid transform");
        assert_eq!(transform.tex_coord, 1);
        let uv = transform.apply([0.25, 0.5]);
        assert!(
            (uv[0] + 1.4).abs() < 1e-6 && (uv[1] - 0.7).abs() < 1e-6,
            "{uv:?}"
        );
        assert_eq!(
            super::transform(&json!({"texCoord":1}))
                .expect("slot selection")
                .tex_coord,
            1
        );
        assert_eq!(
            super::transform(&Value::Null).expect("defaults"),
            TextureTransform::default()
        );
        assert!(super::transform(&json!({"texCoord":-1})).is_err());
        assert_eq!(
            super::transform(
                &json!({"texCoord":1,"extensions":{"KHR_texture_transform":{"texCoord":null}}})
            )
            .expect("omitted optional selector")
            .tex_coord,
            1
        );
        assert!(
            super::transform(&json!({"extensions":{"KHR_texture_transform":{"offset":[1]}}}))
                .is_err()
        );
    }
}
