//! The six satin graphite materials (D31, `docs/design/theme.md`): every surface is one of
//! them, so depth, not stripes or outlines, says what a thing is and what state it is in.
//! `Material::Raised.paint(l, row())` gives a box its fill, inner light and shade, and outer
//! edge; callers add size, radius and handlers.
use stillkit::el::El;
use stillkit::{Rgba, SoftShadow};

use crate::look::Look;

/// A satin material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// Set into the surface: fields, tracks, code. Shade under its lip, light below it.
    Well,
    /// Lifted out: buttons, pop-ups, keycaps, segment thumbs, the chosen row. Light on top,
    /// shade along the bottom, a dark edge around.
    Raised,
    /// Pushed in: latched toggles, the active tool, a Raised control while pressed.
    Pressed,
    /// The one main action and on states: white ink's fill, lifted like Raised.
    Primary,
    /// Menus, toasts, dialogs: translucent, ringed and cast well above the page.
    Float,
    /// Panels: the card fill under a faint top light.
    Dock,
}

impl Material {
    /// Paints `el` as this material: fill, inner edges, outer edge or cast shadow, and the
    /// state looks that go with it (Raised lightens under the pointer and presses in).
    pub fn paint<S: 'static>(self, l: &Look, el: El<S>) -> El<S> {
        let c = &l.c;
        let clear = Rgba::TRANSPARENT;
        let ring = |color: Rgba| SoftShadow {
            blur: 0.0,
            offset: (0.0, 0.0),
            spread: 1.0,
            color,
        };
        // The light a well catches just below it (`0 1px 0 well-lip`).
        let lip = SoftShadow {
            blur: 0.0,
            offset: (0.0, 1.0),
            spread: 0.0,
            color: c.well_lip.0,
        };
        match self {
            Material::Well => el
                .fill(c.well.0)
                .edges(c.well_shade.0, clear)
                .box_shadow(lip),
            Material::Raised => {
                let (well, shade, lowlight, hover) = (
                    c.well.0,
                    c.well_shade.0,
                    c.edge_lowlight.0,
                    c.raised_hover.0,
                );
                el.fill(c.raised.0)
                    .edges(c.edge_highlight.0, lowlight)
                    .box_shadow(ring(c.edge_shadow.0))
                    .hovered(move |e| e.fill(hover))
                    .pressed(move |e| e.fill(well).edges(shade, lowlight).box_shadow(lip))
            }
            Material::Pressed => el
                .fill(c.well.0)
                .edges(c.well_shade.0, clear)
                .border(1.0, c.edge_lowlight.0)
                .box_shadow(lip),
            Material::Primary => el
                .fill(c.primary.0)
                .color(c.primary_foreground.0)
                .edges(c.primary_highlight.0, c.primary_lowlight.0)
                .box_shadow(ring(c.edge_shadow.0)),
            Material::Float => el
                .fill(c.menu_surface.0)
                .edges(c.edge_highlight.0, clear)
                .border(1.0, c.float_ring.0)
                .box_shadow(SoftShadow {
                    blur: 34.0,
                    offset: (0.0, 14.0),
                    spread: 0.0,
                    color: c.float_shadow.0,
                }),
            Material::Dock => el.fill(c.card.0).edges(c.dock_highlight.0, clear),
        }
    }
}
