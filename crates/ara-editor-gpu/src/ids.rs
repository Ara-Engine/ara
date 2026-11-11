//! Object ids as the Scene overlays and the click read them, from either renderer.
//!
//! Both sources carry the same compact id: `index + 1` into the renderer's entities,
//! which the editor maps to scene entities and outline groups; 0 is nothing.
//!
//! - **Direct** (ara-render): an R32_SFLOAT image holding the id as a float.
//! - **Packed** (Renderlab): an RGBA8 UNORM (not sRGB) image, the Scene view's output
//!   size, one texel per output pixel, read with an exact texel fetch (never filtered).
//!   The 32-bit id is little-endian across the channels: R bits 0 to 7, G 8 to 15, B 16
//!   to 23, A 24 to 31. [`NO_ID_HIT`] is a hit without an id.
//!
//! The shaders decode both in `readId` (`shaders/ui_body.slang`); this is the CPU side
//! of the same contract (clicks, tests).

/// A packed texel meaning something is there but it is not selectable: it occludes the
/// grid and is never outlined or hovered.
pub const NO_ID_HIT: u32 = 0xFFFF_FFFF;

/// What one id texel says about its pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdHit {
    /// Background: nothing was drawn.
    Nothing,
    /// Something was drawn but it has no selectable id.
    Unselectable,
    /// A selectable object, by compact index + 1 (`ara_render::Renderer::entities()`).
    Id(u32),
}

impl IdHit {
    /// The hit a packed RGBA8 texel (bytes as stored: R, G, B, A) holds.
    pub fn from_packed(texel: [u8; 4]) -> Self {
        match u32::from_le_bytes(texel) {
            0 => Self::Nothing,
            NO_ID_HIT => Self::Unselectable,
            id => Self::Id(id),
        }
    }

    /// The hit an ara-render id texel (the id as a float) holds.
    pub fn from_direct(texel: f32) -> Self {
        match texel as u32 {
            0 => Self::Nothing,
            id => Self::Id(id),
        }
    }

    /// The selectable id, if any.
    pub fn id(self) -> Option<u32> {
        match self {
            Self::Id(id) => Some(id),
            _ => None,
        }
    }
}

/// Where the Scene overlays read object ids. Each handle names an external image
/// attached with `NativeUi::external_image`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdSource {
    /// ara-render's id image (R32_SFLOAT, the id as a float).
    Direct(u32),
    /// A packed id image, as Renderlab delivers it (see the module docs).
    Packed(u32),
    /// Debug: an ara-render id image repacked on the GPU into the packed form before the
    /// overlays run, so every overlay takes the packed path over the same ids as
    /// `Direct`. Metal only; Vulkan refuses it.
    Repacked(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_texels_are_little_endian() {
        assert_eq!(IdHit::from_packed([0; 4]), IdHit::Nothing);
        assert_eq!(IdHit::from_packed([0xFF; 4]), IdHit::Unselectable);
        assert_eq!(
            IdHit::from_packed([0x01, 0x02, 0x03, 0x04]),
            IdHit::Id(0x0403_0201)
        );
        assert_eq!(
            IdHit::from_packed([0xFE, 0xFF, 0xFF, 0xFF]),
            IdHit::Id(0xFFFF_FFFE)
        );
    }

    #[test]
    fn direct_texels_hold_the_id_as_a_float() {
        assert_eq!(IdHit::from_direct(0.), IdHit::Nothing);
        assert_eq!(IdHit::from_direct(7.), IdHit::Id(7));
        assert_eq!(IdHit::from_direct(7.).id(), Some(7));
        assert_eq!(IdHit::Unselectable.id(), None);
    }
}
