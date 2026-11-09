//! The studio's surfaces. Panels are told apart by tone, never by lines (Unreal, Unity):
//! the window's gaps are darkest, a tab strip sits a step above them with its active tab
//! joined to the panel body, component headers are a band a step above the body, and
//! fields are wells set into it. Everything comes from the theme's tokens.
use ara_kit::look::Look;
use ara_kit::ui::mix;
use stillkit::Rgba;

/// Corner radius of panels, bands, wells and tabs: tools, not cards.
pub const RADIUS: f32 = 4.0;
/// A tree, list or field row.
pub const ROW: f32 = 20.0;
/// A panel's tab strip.
pub const STRIP: f32 = 26.0;
/// A toolbar inside a panel.
pub const TOOLBAR: f32 = 26.0;
/// Body text.
pub const TEXT: f32 = 12.0;

/// The studio's surface tones for the current scheme.
#[derive(Clone, Copy, Debug)]
pub struct Tones {
    /// Between panels, behind everything.
    pub gap: Rgba,
    /// A panel's tab strip and toolbars.
    pub strip: Rgba,
    /// A panel's body (and its active tab).
    pub panel: Rgba,
    /// Headers inside a panel (components, groups).
    pub band: Rgba,
    /// Fields and search boxes, set into the panel.
    pub well: Rgba,
    /// A row or control under the pointer.
    pub hover: Rgba,
}

/// The tones for `l`'s scheme.
pub fn tones(l: &Look) -> Tones {
    let c = &l.c;
    if l.dark {
        Tones {
            gap: c.chrome.0,
            strip: mix(c.chrome.0, c.card.0, 0.4),
            panel: mix(c.card.0, c.raised.0, 0.45),
            band: c.raised.0,
            well: l.well(),
            hover: c.hover_surface.0,
        }
    } else {
        Tones {
            gap: c.chrome.0,
            strip: mix(c.chrome.0, c.card.0, 0.45),
            panel: c.card.0,
            band: c.raised.0,
            well: l.well(),
            hover: c.hover_surface.0,
        }
    }
}
