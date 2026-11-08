//! Ara's look: the semantic tokens of `assets/themes/ara-{dark,light}.json` (D18) as typed
//! values, and the stillkit [`Theme`] they make. Views read Ara's own tokens through
//! [`Look`] (`look.c.selection_surface`); stillkit and stillkit-kit components read the mapped
//! theme (`token::LABEL` is `foreground`), so both agree in either scheme.
use serde::Deserialize;
use stillkit::{Rgba, Scheme, Theme};

/// Every colour token, by its JSON name (`muted-foreground` is `muted_foreground`).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Colors {
    /// The app background behind panels.
    pub background: Hex,
    /// Scrims behind dialogs.
    pub overlay: Hex,
    /// Primary text.
    pub foreground: Hex,
    /// Muted panel fill.
    pub muted: Hex,
    /// Secondary text.
    pub muted_foreground: Hex,
    /// Panels and cards.
    pub card: Hex,
    /// Text on cards.
    pub card_foreground: Hex,
    /// Popovers.
    pub popover: Hex,
    /// Text on popovers.
    pub popover_foreground: Hex,
    /// Secondary buttons.
    pub secondary: Hex,
    /// Text on secondary buttons.
    pub secondary_foreground: Hex,
    /// Hairlines.
    pub border: Hex,
    /// Field wells.
    pub input: Hex,
    /// Focus rings.
    pub ring: Hex,
    /// Primary actions.
    pub primary: Hex,
    /// Text on primary actions.
    pub primary_foreground: Hex,
    /// The accent.
    pub accent: Hex,
    /// Text on the accent.
    pub accent_foreground: Hex,
    /// Soft accent surfaces (badges, focus washes).
    pub accent_surface: Hex,
    /// Text on soft accent surfaces.
    pub accent_surface_foreground: Hex,
    /// Title bar and window chrome.
    pub chrome: Hex,
    /// Controls one step off a panel.
    pub raised: Hex,
    /// Panel headers.
    pub panel_header: Hex,
    /// Window borders and dock splitters.
    pub separator_strong: Hex,
    /// Selected rows.
    pub selection_surface: Hex,
    /// Text on selected rows.
    pub selection_foreground: Hex,
    /// Secondary text on selected rows.
    pub selection_muted_foreground: Hex,
    /// Selection in an unfocused panel.
    pub selection_surface_inactive: Hex,
    /// Neutral hover.
    pub hover_surface: Hex,
    /// Menus.
    pub menu_surface: Hex,
    /// Shortcut chips.
    pub keycap: Hex,
    /// Errors.
    pub destructive: Hex,
    /// Error text.
    pub destructive_foreground: Hex,
    /// Error tints.
    pub destructive_surface: Hex,
    /// Warnings.
    pub warning: Hex,
    /// Warning text.
    pub warning_foreground: Hex,
    /// Warning tints.
    pub warning_surface: Hex,
    /// Success.
    pub success: Hex,
    /// Success text.
    pub success_foreground: Hex,
    /// Ink on colour swatches.
    pub swatch_ink: Hex,
    /// Ink on dark swatches.
    pub swatch_ink_inverse: Hex,
    /// Information.
    pub info: Hex,
    /// Information text.
    pub info_foreground: Hex,
    /// Code blocks.
    pub code_background: Hex,
    /// Code text.
    pub code_foreground: Hex,
    /// Chart series 1 (the accent).
    pub chart_1: Hex,
    /// Chart series 2.
    pub chart_2: Hex,
    /// Chart series 3.
    pub chart_3: Hex,
    /// Chart series 4.
    pub chart_4: Hex,
    /// Chart series 5.
    pub chart_5: Hex,
    /// Chart series 6.
    pub chart_6: Hex,
}

/// A `#rrggbb` or `#rrggbbaa` colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hex(pub Rgba);

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_hex(&s).map(Hex).ok_or_else(|| serde::de::Error::custom(format!("not a #rrggbb[aa] colour: {s}")))
    }
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    let alpha = match h.len() {
        6 => 255,
        8 => byte(6)?,
        _ => return None,
    };
    Some(Rgba::rgb8([byte(0)?, byte(2)?, byte(4)?], f32::from(alpha) / 255.0))
}

/// Corner radii, design px.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Radius {
    /// Cards and dialogs.
    pub base: f32,
    /// Panels and menus.
    pub md: f32,
    /// Controls.
    pub sm: f32,
    /// Chips and dense controls.
    pub xs: f32,
}

/// Sizes, design px.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Metrics {
    /// Control height.
    pub control_size: f32,
    /// Icon size.
    pub icon_size: f32,
    /// Inset of viewport overlays.
    pub viewport_inset: f32,
    /// Inset of viewport widgets.
    pub viewport_widget_inset: f32,
    /// Top of viewport widgets.
    pub viewport_widget_top: f32,
    /// Body text.
    pub font_size_body: f32,
    /// Small text.
    pub font_size_small: f32,
    /// Headings.
    pub font_size_heading: f32,
    /// Spacing steps.
    pub space_xs: f32,
    /// Spacing steps.
    pub space_sm: f32,
    /// Spacing steps.
    pub space_md: f32,
    /// Spacing steps.
    pub space_lg: f32,
    /// Inside panels.
    pub panel_padding: f32,
    /// Panel header height.
    pub panel_header_size: f32,
    /// Toolbar height.
    pub toolbar_size: f32,
    /// Tab height.
    pub tab_size: f32,
    /// Status bar height.
    pub statusbar_size: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    based_on: String,
    colors: Colors,
    radius: Radius,
    metrics: Metrics,
}

/// One scheme of Ara's look: its tokens and the stillkit theme they make.
#[derive(Clone)]
pub struct Look {
    /// Dark or light.
    pub dark: bool,
    /// Colour tokens.
    pub c: Colors,
    /// Radii.
    pub radius: Radius,
    /// Sizes.
    pub m: Metrics,
    /// The stillkit theme mapped from them.
    pub theme: Theme,
}

const DARK: &str = include_str!("../../../assets/themes/ara-dark.json");
const LIGHT: &str = include_str!("../../../assets/themes/ara-light.json");

impl Look {
    /// The built-in dark (`true`) or light scheme.
    pub fn new(dark: bool) -> Self {
        Self::parse(if dark { DARK } else { LIGHT }, dark).expect("the bundled theme tokens parse")
    }

    /// A scheme from theme JSON (the bundled files, or an edited copy while live-styling).
    pub fn parse(json: &str, dark: bool) -> anyhow::Result<Self> {
        let file: File = serde_json::from_str(json)?;
        let theme = theme(&file.colors, &file.radius, &file.metrics, dark);
        Ok(Self {
            dark,
            c: file.colors,
            radius: file.radius,
            m: file.metrics,
            theme,
        })
    }
}

/// The stillkit theme of Ara's tokens: Solid geometry, the platform's UI face at Ara's dense
/// sizes, and the palette mapped token by token.
fn theme(c: &Colors, r: &Radius, m: &Metrics, dark: bool) -> Theme {
    let scheme = if dark { Scheme::Dark } else { Scheme::Light };
    let mut t = Theme::new(stillkit::Style::Solid, scheme, c.primary.0);
    let p = &mut t.color;
    p.label = c.foreground.0;
    p.secondary = c.muted_foreground.0;
    p.tertiary = c.muted_foreground.0.with_alpha(0.72);
    p.quaternary = c.muted_foreground.0.with_alpha(0.45);
    p.fill = c.raised.0;
    p.card = c.card.0;
    p.well = c.input.0;
    p.separator = c.border.0;
    p.accent = c.primary.0;
    p.on_accent = c.primary_foreground.0;
    p.accent_wash = c.primary.0.with_alpha(0.16);
    p.destructive = c.destructive.0;
    p.success = c.success.0;
    p.warning = c.warning.0;
    p.surface = c.background.0;
    p.elevated = c.menu_surface.0;
    p.hover = c.hover_surface.0.with_alpha(0.7);
    p.pressed = c.hover_surface.0;
    p.knob = Rgba::new(1.0, 1.0, 1.0, 1.0);
    p.scrollbar = c.muted_foreground.0.with_alpha(0.45);
    // Shadows deepen on dark, where surfaces barely step.
    p.shadow = Rgba::new(0.0, 0.0, 0.0, if dark { 0.55 } else { 0.18 });
    t.radius.surface = r.base;
    t.radius.card = r.md;
    t.radius.control = Some(r.sm);
    t.radius.small = r.xs;
    t.metrics.pad = m.panel_padding;
    t.metrics.gap = m.space_lg;
    t.metrics.item = m.space_sm;
    t.metrics.control = m.control_size;
    t.metrics.control_small = m.control_size - 4.0;
    t.metrics.icon = m.icon_size;
    t.metrics.icon_small = m.icon_size - 2.0;
    t.metrics.icon_large = m.icon_size + 4.0;
    t.metrics.row = 36.0;
    t.metrics.row_single = m.control_size;
    t.metrics.hit = m.control_size;
    t.metrics.scrollbar = 6.0;
    t.metrics.scrollbar_inset = 2.0;
    t.border = Some(c.border.0);
    t.font = type_ramp(m);
    t.focus_ring.width = 1.5;
    t.focus_ring.gap = 1.5;
    t
}

/// Dense type for a pro tool: 12 px body, 11 px small text, headings at Ara's sizes.
fn type_ramp(m: &Metrics) -> stillkit::theme::TypeRamp {
    use stillkit::theme::Font;
    let f = |size: f32, weight: u16, line: f32| Font { size, weight, tracking: 0.0, line };
    let body = m.font_size_body;
    let small = m.font_size_small;
    let heading = m.font_size_heading;
    stillkit::theme::TypeRamp {
        family: stillkit::VARIABLE.into(),
        roles: [
            f(28.0, 650, 34.0),              // LargeTitle: page titles
            f(22.0, 650, 28.0),              // Title1
            f(18.0, 600, 24.0),              // Title2: dialog titles
            f(heading, 600, heading + 6.0),  // Title3: section titles
            f(body + 1.0, 560, body + 7.0),  // Headline: row and card titles
            f(body, 400, body + 6.0),        // Body
            f(body, 400, body + 6.0),        // Callout
            f(small, 560, small + 5.0),      // Subheadline: section labels
            f(small, 400, small + 5.0),      // Footnote
            f(small - 1.0, 500, small + 3.0), // Caption: badges, counts
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_bundled_schemes_parse_every_token() {
        let dark = Look::new(true);
        let light = Look::new(false);
        assert_eq!(dark.c.background.0, Rgba::rgb8([0x0a, 0x0a, 0x0a], 1.0));
        assert_eq!(dark.c.overlay.0.a, f32::from(0x88_u8) / 255.0);
        assert!(dark.theme.dark() && !light.theme.dark());
        assert_eq!(dark.theme.color.label, dark.c.foreground.0);
        assert_eq!(light.theme.metrics.control, light.m.control_size);
    }

    #[test]
    fn colours_must_be_hex() {
        assert!(parse_hex("#0a0b0c").is_some());
        assert!(parse_hex("#0a0b0c80").is_some());
        assert!(parse_hex("0a0b0c").is_none() && parse_hex("#0a0b").is_none() && parse_hex("#zz0000").is_none());
    }
}
