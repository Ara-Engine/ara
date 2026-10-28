//! Native material ABI. Texture IDs address a residency table, never an atlas.
use super::*;

/// Material slot order in the native shader ABI.
#[derive(Debug, Clone, Copy)]
#[repr(usize)]
pub enum TextureSlot {
    /// sRGB base colour and linear alpha.
    BaseColor,
    /// Tangent-space normal.
    Normal,
    /// Linear green roughness and blue metallic.
    MetallicRoughness,
    /// sRGB emissive colour.
    Emissive,
    /// Linear red ambient occlusion, for indirect lighting only.
    Occlusion,
    /// Linear red transmission weight.
    Transmission,
}

/// Independent glTF texture coordinates: translation * rotation * scale.
/// Generated normal tangents already match these coordinates; consumers do not
/// apply this rotation to the tangent basis a second time.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TextureTransform {
    /// Translation after scale and rotation.
    pub offset: [f32; 2],
    /// Scale before rotation.
    pub scale: [f32; 2],
    /// Counterclockwise rotation in radians.
    pub rotation: f32,
    /// Selected coordinate set, zero or one.
    pub tex_coord: u32,
    /// Reserved shader words; must be zero.
    pub reserved: [u32; 2],
}

impl Default for TextureTransform {
    fn default() -> Self {
        Self {
            offset: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
            tex_coord: 0,
            reserved: [0; 2],
        }
    }
}

impl TextureTransform {
    /// Applies the selected coordinate set without altering the source attributes.
    pub fn apply(&self, uv0: [f32; 2], uv1: [f32; 2]) -> [f32; 2] {
        let uv = if self.tex_coord == 0 { uv0 } else { uv1 };
        let [x, y] = [uv[0] * self.scale[0], uv[1] * self.scale[1]];
        let (sin, cos) = self.rotation.sin_cos();
        [
            self.offset[0] + cos * x - sin * y,
            self.offset[1] + sin * x + cos * y,
        ]
    }
}

/// Native extensions to the baseline PBR block, exactly 240 bytes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialExtras {
    /// Independent coordinates in [`TextureSlot`] order.
    pub uv: [TextureTransform; 6],
    /// Residency-table texture ID, or [`NO_TEXTURE`] for white.
    pub occlusion_texture: u32,
    /// Residency-table texture ID, or [`NO_TEXTURE`] for white.
    pub transmission_texture: u32,
    /// Zero for an ordinary surface; one to four for authored terrain splats.
    pub terrain_layers: u32,
    /// One reconstructs a normal basis from triangle coordinates after a material
    /// override changes normal UVs; zero uses supplied/generated tangents as authored.
    pub reconstruct_normal_basis: u32,
    /// Mix of unoccluded and authored occlusion, from zero to one.
    pub occlusion_strength: f32,
    /// Fraction transmitted before the red transmission-map multiplier.
    pub transmission_factor: f32,
    /// A transmissive material's index of refraction and volume, packed by
    /// [`Volume::pack`] (shaders/look-refract.slangh decodes them); all zero is
    /// glTF's default: IOR 1.5, a thin wall, no absorption.
    pub volume: [u32; 2],
    /// Material IDs for the terrain's weight channels, in Header.materials order.
    /// Unused channels are [`NO_TEXTURE`]; layer materials cannot contain splats.
    pub layer_materials: [u32; 4],
}

impl Default for MaterialExtras {
    fn default() -> Self {
        Self {
            uv: [TextureTransform::default(); 6],
            occlusion_texture: NO_TEXTURE,
            transmission_texture: NO_TEXTURE,
            terrain_layers: 0,
            reconstruct_normal_basis: 0,
            occlusion_strength: 1.0,
            transmission_factor: 0.0,
            volume: [0; 2],
            layer_materials: [NO_TEXTURE; 4],
        }
    }
}

/// glTF KHR_materials_ior and KHR_materials_volume of a transmissive material:
/// refraction and absorption in the forward pass (`native_forward_refract`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume {
    /// Index of refraction, clamped to 1..3 when packed.
    pub ior: f32,
    /// Thickness of the volume beneath the surface, in the mesh's local units (the
    /// instance scale applies); zero is a thin-walled surface, which bends no rays.
    pub thickness: f32,
    /// Linear colour white light keeps after `attenuation_distance` in the medium.
    pub attenuation_color: [f32; 3],
    /// World distance (m) at which light keeps `attenuation_color`; infinite: clear.
    pub attenuation_distance: f32,
}

impl Default for Volume {
    fn default() -> Self {
        Self {
            ior: 1.5,
            thickness: 0.,
            attenuation_color: [1.; 3],
            attenuation_distance: f32::INFINITY,
        }
    }
}

impl Volume {
    /// Packs into [`MaterialExtras::volume`]: word 0 holds the IOR (high 16 bits:
    /// zero for 1.5, else 1 + (code - 1) / 32767) and the thickness (low 16 bits, an
    /// IEEE half); word 1 the absorption coefficient per metre, -ln(colour) /
    /// distance, as RGB9E5.
    pub fn pack(&self) -> [u32; 2] {
        let ior = if self.ior == 1.5 || !self.ior.is_finite() {
            0
        } else {
            1 + ((self.ior.clamp(1., 3.) - 1.) * 32767.).round() as u32
        };
        let absorption = self.attenuation_color.map(|c| {
            if self.attenuation_distance.is_finite() && self.attenuation_distance > 0. {
                -c.clamp(1e-6, 1.).ln() / self.attenuation_distance
            } else {
                0.
            }
        });
        [(ior << 16) | half(self.thickness), rgb9e5(absorption)]
    }

    /// The IOR, thickness and per-metre absorption coefficients `pack` stored.
    pub fn unpack(words: [u32; 2]) -> (f32, f32, [f32; 3]) {
        let code = words[0] >> 16;
        let ior = if code == 0 {
            1.5
        } else {
            1. + (code - 1) as f32 / 32767.
        };
        let h = words[0] & 0xffff;
        let thickness = if h < 0x400 {
            h as f32 * 2f32.powi(-24)
        } else {
            f32::from_bits((((h >> 10) + 112) << 23) | ((h & 0x3ff) << 13))
        };
        let scale = 2f32.powi((words[1] >> 27) as i32 - 24);
        let absorption = [0, 9, 18].map(|shift| ((words[1] >> shift) & 511) as f32 * scale);
        (ior, thickness, absorption)
    }
}

/// A non-negative finite value as IEEE half bits, rounded to nearest (saturating).
fn half(value: f32) -> u32 {
    let value = if value.is_finite() {
        value.clamp(0., 65504.)
    } else {
        0.
    };
    if value < 2f32.powi(-14) {
        return (value * 2f32.powi(24)).round() as u32;
    }
    let bits = value.to_bits();
    let exponent = (bits >> 23) - 112;
    ((exponent << 10) + (((bits & 0x7f_ffff) + 0x1000) >> 13)).min(0x7bff)
}

/// Non-negative RGB as shared-exponent RGB9E5 (EXT_texture_shared_exponent).
fn rgb9e5(rgb: [f32; 3]) -> u32 {
    const MAX: f32 = 511. / 512. * 65536.;
    let rgb = rgb.map(|v| if v.is_finite() { v.clamp(0., MAX) } else { MAX });
    let peak = rgb[0].max(rgb[1]).max(rgb[2]);
    if peak <= 0. {
        return 0;
    }
    let mut exponent = (peak.log2().floor() as i32).max(-16) + 16;
    let mut scale = 2f32.powi(exponent - 24);
    if (peak / scale).round() >= 512. {
        exponent += 1;
        scale *= 2.;
    }
    let [r, g, b] = rgb.map(|v| ((v / scale).round() as u32).min(511));
    r | (g << 9) | (b << 18) | ((exponent as u32) << 27)
}

/// PBR factors plus six independent map channels, exactly 320 bytes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Material {
    /// Baseline factors and base/normal/metallic-roughness/emissive texture IDs.
    /// IDs address native residency rows; [`NO_TEXTURE`] supplies neutral texels.
    pub pbr: crate::scene::Material,
    /// Occlusion, transmission, coordinate transforms and terrain splats.
    pub extras: MaterialExtras,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            pbr: crate::scene::Material {
                textures: [NO_TEXTURE; 4],
                ..crate::scene::Material::default()
            },
            extras: MaterialExtras::default(),
        }
    }
}
