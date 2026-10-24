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
    /// Reserved shader words; must be zero.
    pub reserved: [u32; 2],
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
            reserved: [0; 2],
            layer_materials: [NO_TEXTURE; 4],
        }
    }
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
