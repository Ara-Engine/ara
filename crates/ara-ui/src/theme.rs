//! D18 semantic tokens; the canonical palette files are embedded without duplication.
use crate::{Result, Ui};
use serde::Deserialize;
use std::collections::BTreeMap;
/// Built-in editor palette, switchable without rebuilding the UI document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorTheme {
    /// T3 Code's default dark palette.
    #[default]
    Dark,
    /// T3 Code's default light palette.
    Light,
}
#[derive(Deserialize)]
struct Palette {
    colors: BTreeMap<String, String>,
    radius: BTreeMap<String, f32>,
    #[serde(default)]
    metrics: BTreeMap<String, f32>,
}
impl EditorTheme {
    /// Embedded JSON resource for this built-in theme.
    pub fn json(self) -> &'static str {
        match self {
            Self::Dark => include_str!("../../../assets/themes/ara-dark.json"),
            Self::Light => include_str!("../../../assets/themes/ara-light.json"),
        }
    }
    /// CSS custom properties for semantic colors and radius tokens (parsed once per
    /// process and theme; a theme switch only swaps them).
    pub fn tokens(self) -> Result<BTreeMap<String, String>> {
        static PARSED: [std::sync::OnceLock<
            std::result::Result<BTreeMap<String, String>, String>,
        >; 2] = [std::sync::OnceLock::new(), std::sync::OnceLock::new()];
        PARSED[self as usize]
            .get_or_init(|| self.parse_tokens().map_err(|e| e.to_string()))
            .clone()
            .map_err(Into::into)
    }
    fn parse_tokens(self) -> Result<BTreeMap<String, String>> {
        tokens_from_json(self.json())
    }
}
/// Parse a theme resource into CSS variables. Metrics and radii are finite,
/// nonnegative pixel values; colors retain their semantic names.
pub fn tokens_from_json(json: &str) -> Result<BTreeMap<String, String>> {
    let p: Palette = serde_json::from_value(ara_json::parse(json)?)?;
    for (name, color) in &p.colors {
        crate::style::Color::parse(color).map_err(|e| format!("theme color {name}: {e}"))?;
    }
    let mut out: BTreeMap<_, _> = p
        .colors
        .into_iter()
        .map(|(k, v)| (format!("--{k}"), v))
        .collect();
    out.extend(
        p.radius
            .into_iter()
            .map(|(k, v)| (format!("radius-{k}"), v))
            .chain(p.metrics)
            .map(|(k, v)| {
                if !v.is_finite() || v < 0. {
                    return Err(format!("invalid theme metric {k}: {v}").into());
                }
                Ok((format!("--{k}"), format!("{v}px")))
            })
            .collect::<Result<BTreeMap<_, _>>>()?,
    );
    // Older palette files omit two status surfaces. Derive their documented
    // tint from the same canonical foreground/background tokens.
    for status in ["success", "info"] {
        let key = format!("--{status}-surface");
        if !out.contains_key(&key) {
            let parse = |name: &str| -> Result<u32> {
                Ok(u32::from_str_radix(
                    out.get(name)
                        .ok_or("missing status color")?
                        .trim_start_matches('#'),
                    16,
                )?)
            };
            let base = parse("--background")?;
            let tint = parse(&format!("--{status}-foreground"))?;
            let channel = |shift: u32| {
                (((base >> shift) & 255u32) as f32 * 0.88
                    + ((tint >> shift) & 255u32) as f32 * 0.12)
                    .round() as u8
            };
            out.insert(
                key,
                format!("#{:02x}{:02x}{:02x}", channel(16), channel(8), channel(0)),
            );
        }
    }
    Ok(out)
}
impl Ui {
    /// Parse editor markup with Ara Dark defaults and raw-color diagnostics.
    pub fn editor(markup: &str, css: &str, size: [f32; 2]) -> Result<Self> {
        check_editor(markup, css)?;
        let mut ui = Self::with_tokens(markup, css, size, EditorTheme::Dark.tokens()?)?;
        ui.editor_checks = true;
        Ok(ui)
    }
    /// Swap the palette tokens and restyle once on the next frame; color-only changes
    /// do not reshape or relayout. The current theme again is a no-op.
    pub fn switch_theme(&mut self, theme: EditorTheme) -> Result<()> {
        self.set_theme_tokens(theme.tokens()?);
        Ok(())
    }
    /// Apply validated resource tokens to the retained tree. Restyling detects
    /// geometry and font changes and invalidates layout or shaping as needed.
    pub fn set_theme_tokens(&mut self, tokens: BTreeMap<String, String>) -> bool {
        let changed = self.sheet.set_tokens(tokens);
        if changed {
            for n in &mut self.nodes {
                n.dirty.style = true;
            }
        }
        changed
    }
}
/// Validate both inline styles and stylesheet rules against D18.
pub fn check_editor(markup: &str, css: &str) -> Result<()> {
    let sheet = ara_ui_document::stylesheet::Stylesheet::parse(css)?;
    sheet.check_editor(css)?;
    check_inline(markup)
}
/// Reject raw colors in editor inline styles (D18: semantic tokens only).
pub(crate) fn check_inline(markup: &str) -> Result<()> {
    for block in ara_ui_document::inline_styles(markup)? {
        for d in ara_ui_document::stylesheet::declarations(&block.value, 0)? {
            if raw_color(&d.value) {
                return Err(ara_ui_document::Diagnostic::new(
                    ara_ui_document::codes::RAW_COLOR,
                    markup,
                    block.bytes,
                    format!("Raw color in editor node {}", block.id),
                    "Replace with a semantic var(--token)",
                )
                .into());
            }
        }
    }
    Ok(())
}
/// One editor node's inline style against D18 (what `check_inline` does for a whole
/// document); reconciles check only new and restyled nodes this way.
pub(crate) fn check_node_style(element: &ara_ui_document::Element) -> Result<()> {
    let Some(style) = element.attrs.get("style") else {
        return Ok(());
    };
    for d in ara_ui_document::stylesheet::declarations(style, 0)? {
        if raw_color(&d.value) {
            return Err(ara_ui_document::Diagnostic::new(
                ara_ui_document::codes::RAW_COLOR,
                style,
                d.bytes,
                format!("Raw color in editor node {}", element.id),
                "Replace with a semantic var(--token)",
            )
            .into());
        }
    }
    Ok(())
}
fn raw_color(value: &str) -> bool {
    value.contains('#') || value.contains("rgb(") || value.contains("rgba(")
}
/// Compact editor rules shared by fixtures and the upcoming editor shell.
pub const EDITOR_CSS: &str = r#"
ui { width:100%; height:100%; background:var(--background); color:var(--foreground); font-size:13px; padding:12px; gap:8px; }
.screen { width:100%; height:100%; background:var(--background); color:var(--foreground); font-size:13px; padding:12px; gap:8px; }
panel { gap:8px; }
.card { background:var(--card); border-color:var(--border); border-width:1px; border-radius:var(--radius-base); padding:12px; }
.muted { color:var(--muted-foreground); }
.heading { font-size:16px; }
.row { display:grid; grid-template-columns:1fr 1fr; gap:8px; }
button { height:32px; padding:6px; background:var(--secondary); color:var(--foreground); font-size:13px; border-radius:var(--radius-sm); border-width:1px; border-color:var(--border); }
button:hover { background:var(--accent-surface); }
button:active { background:var(--primary); color:var(--primary-foreground); }
button:focus { border-color:var(--ring); }
button:disabled { color:var(--muted-foreground); }
field { height:30px; padding:5px; background:var(--background); color:var(--foreground); font-size:13px; border-radius:var(--radius-xs); border-width:1px; border-color:var(--input); overflow:hidden; }
field:focus { border-color:var(--ring); }
toggle { height:30px; padding:5px; color:var(--foreground); background:var(--secondary); font-size:13px; }
slider { height:26px; background:var(--secondary); color:var(--accent); }
.selected { background:var(--accent-surface); border-color:var(--accent); }
.virtual-row { height:26px; padding:3px; border-radius:0px; }
scroll { overflow:hidden; }
"#;
