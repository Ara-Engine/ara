//! The studio's surfaces. Panels are told apart by tone, never by lines (Unreal, Unity):
//! the window's gaps are darkest, a tab strip sits a step above them with its active tab
//! joined to the panel body, component headers are a band a step above the body, and
//! fields are wells set into it. Everything comes from the theme's tokens.
use ara_kit::look::Look;
use ara_kit::ui::{Material, mix};
use stillkit::Rgba;
use stillkit::el::El;

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

/// `el` in the Raised tone while `on`: how the chosen row, folder, tile or tool shows (a
/// step lighter, never an outline or the accent). Start a chain with it:
/// `chosen(l, row(), selected).key(..)`.
pub fn chosen<S: 'static>(l: &Look, el: El<S>, on: bool) -> El<S> {
    if on {
        Material::Raised.paint(l, el)
    } else {
        el
    }
}

/// A popup's panel (menus, the palette, the Create browser, dialogs): the Float material.
pub fn float<S: 'static>(l: &Look, el: El<S>) -> El<S> {
    Material::Float.paint(l, el)
}

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

/// A drag track for a number in `range` beside its field, as Unreal's sliders: a press
/// or drag sets the value at the pointer through `set`. `log` spreads a wide range (a
/// brush radius from millimetres to metres) evenly by magnitude.
pub fn slider<S: 'static>(
    l: &Look,
    key: String,
    value: f32,
    (min, max): (f32, f32),
    log: bool,
    set: impl Fn(&mut S, f32) + 'static,
) -> El<S> {
    let log = log && min > 0.0 && max > min;
    let along = match log {
        true => (value.max(min) / min).ln() / (max / min).ln(),
        false => (value - min) / (max - min).max(f32::EPSILON),
    }
    .clamp(0.0, 1.0);
    let at = move |t: f32| match log {
        true => min * (max / min).powf(t),
        false => min + (max - min) * t,
    };
    let c = &l.c;
    stillkit::el::row()
        .key(key)
        .h(18.0)
        .align(stillkit::el::Align::Center)
        .cursor(stillkit::Cursor::ResizeHorizontal)
        .on_pointer(move |s: &mut S, e: &stillkit::el::PointerEvent, _| {
            use stillkit::el::PointerPhase;
            if !matches!(e.phase, PointerPhase::Down | PointerPhase::Move) || e.size.0 <= 0.0 {
                return false;
            }
            if e.phase == PointerPhase::Move && e.button != stillkit::Button::Left {
                return false;
            }
            set(s, at((e.local.x / e.size.0).clamp(0.0, 1.0)));
            true
        })
        .child(
            stillkit::el::el()
                .grow(along.max(0.001))
                .h(4.0)
                .radius(2.0)
                .fill(c.primary.0),
        )
        .child(
            stillkit::el::el()
                .size(10.0)
                .radius(5.0)
                .fill(c.foreground.0),
        )
        .child(
            stillkit::el::el()
                .grow((1.0 - along).max(0.001))
                .h(4.0)
                .radius(2.0)
                .fill(c.muted.0),
        )
}
