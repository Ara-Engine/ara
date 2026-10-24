//! Optional material layers over the unchanged `ara-render::scene::Material` ABI.
use anyhow::{Result, ensure};

pub(crate) fn has_layers(materials: &[ara_render::scene::Material]) -> bool {
    materials.iter().any(|m| {
        m.flags[3] == 0x4152_4132 && ((m.flags[1] & 0xff00ff) != 0 || (m.flags[2] & 0xffffff) != 0)
    })
}

/// Typed extraction of coat, cloth and thin-leaf parameters. The reserved flag
/// words carry a version tag; untagged materials retain the original BRDF exactly.
/// This adapter is for extracted snapshots, never an authoring side-channel write.
#[derive(Debug, Clone, Copy)]
pub struct MaterialDetail {
    /// Dielectric clearcoat coverage, from zero to one.
    pub clearcoat: f32,
    /// Perceptual roughness of the coat, from 0.045 to one.
    pub coat_roughness: f32,
    /// Linear cloth sheen reflectance. Zero disables the layer.
    pub sheen: [f32; 3],
    /// Perceptual cloth roughness, from 0.1 to one.
    pub sheen_roughness: f32,
    /// Fraction of diffuse energy transmitted through a two-sided thin sheet.
    pub transmission: f32,
    /// Whether ORM.R contains authored occlusion; never multiply this over traced visibility.
    pub occlusion: bool,
    /// Dielectric F0 multiplier, from zero to one; ORM alpha supplies its map.
    pub specular: f32,
}
impl Default for MaterialDetail {
    fn default() -> Self {
        Self {
            clearcoat: 0.,
            coat_roughness: 0.15,
            sheen: [0.; 3],
            sheen_roughness: 0.5,
            transmission: 0.,
            occlusion: false,
            specular: 1.,
        }
    }
}
impl MaterialDetail {
    /// Encode validated linear parameters into the snapshot's reserved words.
    pub fn apply(self, material: &mut ara_render::scene::Material) -> Result<()> {
        ensure!(
            [self.clearcoat, self.transmission, self.specular]
                .into_iter()
                .chain(self.sheen)
                .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
                && (0.045..=1.0).contains(&self.coat_roughness)
                && (0.1..=1.0).contains(&self.sheen_roughness),
            "Invalid material layer"
        );
        let byte = |v: f32| (v * 255.).round() as u32;
        let color_decode = material.flags[1]
            & (ara_render::scene::BASE_COLOR_DECODE_SRGB | ara_render::scene::EMISSIVE_DECODE_SRGB);
        material.flags[1] = color_decode
            | byte(self.clearcoat)
            | (byte(self.coat_roughness) << 8)
            | (byte(self.transmission) << 16)
            | (u32::from(self.occlusion) << 24);
        material.flags[2] = byte(self.sheen[0])
            | (byte(self.sheen[1]) << 8)
            | (byte(self.sheen[2]) << 16)
            | (byte(self.sheen_roughness) << 24);
        material.flags[3] = 0x4152_4132;
        material.surface[2] = self.specular;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tagged_layers_preserve_alpha_contract_and_reject_invalid_energy() {
        let mut m = ara_render::scene::Material::default();
        m.flags[0] = 1;
        m.flags[1] =
            ara_render::scene::BASE_COLOR_DECODE_SRGB | ara_render::scene::EMISSIVE_DECODE_SRGB;
        MaterialDetail {
            transmission: 0.25,
            ..MaterialDetail::default()
        }
        .apply(&mut m)
        .unwrap();
        assert_eq!(m.flags[0], 1);
        assert_eq!(m.flags[3], 0x4152_4132);
        assert_eq!((m.flags[1] >> 16) & 255, 64);
        assert_ne!(m.flags[1] & ara_render::scene::BASE_COLOR_DECODE_SRGB, 0);
        assert_ne!(m.flags[1] & ara_render::scene::EMISSIVE_DECODE_SRGB, 0);
        assert!(
            MaterialDetail {
                clearcoat: f32::NAN,
                ..MaterialDetail::default()
            }
            .apply(&mut m)
            .is_err()
        );
    }
}
