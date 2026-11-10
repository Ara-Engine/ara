//! Ara's components on stillkit `el`: what the Hub and the editor build pages from. Each takes
//! the current [`Look`] for Ara's tokens and returns an `El` the caller keys, sizes and gives
//! handlers (`.key("save").on_press(..)`), so components stay plain functions.
//!
//! Interaction feedback is layered, never re-run through the view: the runtime's wash (the
//! theme's `hover`/`pressed`, a translucent white or black) eases over any fill; `hovered`
//! looks change ink and borders; focus rings come from the theme.
mod button;
mod chrome;
mod display;
mod field;
mod material;
mod overlay;

pub use button::*;
pub use chrome::*;
pub use display::*;
pub use field::*;
pub use material::*;
pub use overlay::*;

use stillkit::Rgba;

/// `a` moved `t` of the way to `b` (straight sRGB; fine for the small steps components take).
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rgba::new(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b), l(a.a, b.a))
}

/// The font family for values, paths and code: the first monospaced face this system has.
pub fn mono() -> &'static str {
    use std::sync::OnceLock;
    static MONO: OnceLock<&'static str> = OnceLock::new();
    MONO.get_or_init(|| {
        let ui = stillkit::Ui::new().ok();
        let has = |family: &str| ui.as_ref().is_some_and(|ui| ui.text.has_family(family));
        [
            "JetBrains Mono",
            "SF Mono",
            "Cascadia Mono",
            "Menlo",
            "Consolas",
            "DejaVu Sans Mono",
        ]
        .into_iter()
        .find(|f| has(f))
        .unwrap_or("Menlo")
    })
}
