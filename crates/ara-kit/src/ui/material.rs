//! The surfaces every Ara control is built from. The look is flat: a surface is a fill, and
//! tone alone says what it is and what state it is in (no inset light, rims or lifts).
//! Only popups cast a shadow, to separate them from what they cover.
//! `Material::Raised.paint(l, row())` gives a box its fill and state looks; callers add
//! size, radius and handlers.
use stillkit::SoftShadow;
use stillkit::el::El;

use crate::look::Look;

/// A surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// A step into the surface: fields, tracks, code.
    Well,
    /// A step out of it: buttons, pop-up buttons, keycaps, segment thumbs, the chosen row.
    /// A step lighter under the pointer, a well while pressed.
    Raised,
    /// Latched toggles and the active tool: the well's tone.
    Pressed,
    /// The one main action and on states: the primary fill and ink.
    Primary,
    /// Menus, toasts, dialogs: the popover fill over a soft shadow.
    Float,
    /// Panels and cards: the card fill.
    Dock,
}

impl Material {
    /// Paints `el` as this material: its fill and the state looks that go with it.
    pub fn paint<S: 'static>(self, l: &Look, el: El<S>) -> El<S> {
        let c = &l.c;
        match self {
            Material::Well => el.fill(c.well.0),
            Material::Raised => {
                let (hover, well) = (c.raised_hover.0, c.well.0);
                el.fill(c.raised.0)
                    .hovered(move |e| e.fill(hover))
                    .pressed(move |e| e.fill(well))
            }
            Material::Pressed => el.fill(c.well.0),
            Material::Primary => el.fill(c.primary.0).color(c.primary_foreground.0),
            Material::Float => el.fill(c.popover.0).box_shadow(SoftShadow {
                blur: 24.0,
                offset: (0.0, 8.0),
                spread: 0.0,
                color: c.float_shadow.0,
            }),
            Material::Dock => el.fill(c.card.0),
        }
    }
}
