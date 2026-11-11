//! Where the Scene overlays read entity ids from. Outlines, hover and the grid's occlusion all
//! read one id texture (`overlay::ENTITY_IMAGE`) through the UI adapter's single fetch; this is
//! which texture that is and how it encodes ids.
//!
//! - [`IdSource::Renderer`]: ara-render's own id channel, a float texel per render pixel
//!   (compact channel index + 1, 0 for nothing). A raster Scene on it draws the selection
//!   outlines itself; the grid is the only overlay that reads it.
//! - [`IdSource::Packed`]: a packed id image delivered by the renderer (Renderlab): RGBA8
//!   UNORM, one texel per output pixel, the 32-bit id little-endian across R, G, B, A, 0 for
//!   nothing, `u32::MAX` for a hit without an id (it hides the grid, it never outlines).
//!   Whoever delivers it registers it as `ENTITY_IMAGE`.
//! - [`IdSource::Repacked`]: a debug source for machines with no such renderer: every frame
//!   the adapter repacks ara-render's float ids into that packed form on the GPU and reads
//!   the copy (`ARA_PACKED_IDS=1`). Anything the overlays draw must be identical through it.
//!
//! Only `Renderer` leaves the outlines to the renderer; the other sources draw selection and
//! review outlines, and the pointer's hover, as overlays from their ids.
use ara_editor_gpu::{IdEncoding, NativeUi};

use crate::overlay;

/// The environment switch that turns the debug repack on.
const REPACK_VARIABLE: &str = "ARA_PACKED_IDS";

/// Which id texture and encoding the overlays read; see the module docs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum IdSource {
    /// ara-render's own float id channel.
    #[default]
    Renderer,
    /// A packed id image the renderer delivers, registered as `ENTITY_IMAGE`. Nothing
    /// delivers one yet (Renderlab will); `Repacked` stands in for it.
    #[allow(dead_code, reason = "constructed by the Renderlab bridge once it delivers ids")]
    Packed,
    /// ara-render's ids repacked on the GPU each frame (debug).
    Repacked,
}

impl IdSource {
    /// `Repacked` when `ARA_PACKED_IDS` is set to `1`, else `Renderer`.
    pub(crate) fn from_environment() -> Self {
        match std::env::var_os(REPACK_VARIABLE) {
            Some(value) if value == "1" => Self::Repacked,
            _ => Self::Renderer,
        }
    }

    /// Whether a raster Scene draws the selection outlines itself, on its own ids. The
    /// other sources have the editor draw them as overlays.
    pub(crate) fn renderer_draws_outlines(self) -> bool {
        self == Self::Renderer
    }

    /// Whether the pointer's hover is an overlay read from the ids (the renderer-owned path
    /// has no hover overlay).
    pub(crate) fn overlays_hover(self) -> bool {
        self != Self::Renderer
    }

    /// Tells the adapter how to read `ENTITY_IMAGE`.
    pub(crate) fn apply(self, ui: &mut NativeUi) {
        match self {
            Self::Renderer => {
                ui.repack_ids(None);
                ui.set_id_encoding(IdEncoding::Float);
            }
            Self::Packed => {
                ui.repack_ids(None);
                ui.set_id_encoding(IdEncoding::Packed);
            }
            Self::Repacked => ui.repack_ids(Some(overlay::ENTITY_IMAGE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_renderer_source_leaves_outlines_and_hover_to_the_renderer() {
        assert!(IdSource::Renderer.renderer_draws_outlines());
        assert!(!IdSource::Renderer.overlays_hover());
        for source in [IdSource::Packed, IdSource::Repacked] {
            assert!(!source.renderer_draws_outlines(), "{source:?}");
            assert!(source.overlays_hover(), "{source:?}");
        }
    }
}
