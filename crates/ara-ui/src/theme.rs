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
}
impl EditorTheme {
    /// Exact canonical JSON installed by the parent task.
    pub fn json(self) -> &'static str {
        match self {
            Self::Dark => include_str!("../../../assets/themes/ara-dark.json"),
            Self::Light => include_str!("../../../assets/themes/ara-light.json"),
        }
    }
    /// CSS custom properties for semantic colors and radius tokens.
    pub fn tokens(self) -> Result<BTreeMap<String, String>> {
        let p: Palette = serde_json::from_str(self.json())?;
        let mut out: BTreeMap<_, _> = p
            .colors
            .into_iter()
            .map(|(k, v)| (format!("--{k}"), v))
            .collect();
        out.extend(
            p.radius
                .into_iter()
                .map(|(k, v)| (format!("--radius-{k}"), format!("{v}px"))),
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
}
impl Ui {
    /// Parse editor markup with Ara Dark defaults and raw-color diagnostics.
    pub fn editor(markup: &str, css: &str, size: [f32; 2]) -> Result<Self> {
        check_editor(markup, css)?;
        Self::with_tokens(markup, css, size, EditorTheme::Dark.tokens()?)
    }
    /// Recompute only style differences; color-only changes do not reshape or relayout.
    pub fn switch_theme(&mut self, theme: EditorTheme) -> Result<()> {
        self.sheet = crate::style::Sheet::with_tokens(&self.stylesheet_source, theme.tokens()?)?;
        for n in &mut self.nodes {
            n.dirty.style = true;
        }
        Ok(())
    }
}
/// Validate both inline styles and stylesheet rules against D18.
pub fn check_editor(markup: &str, css: &str) -> Result<()> {
    let sheet = ara_ui_document::stylesheet::Stylesheet::parse(css)?;
    sheet.check_editor(css)?;
    for block in ara_ui_document::inline_styles(markup)? {
        for d in ara_ui_document::stylesheet::declarations(&block.value, 0)? {
            if d.value.contains('#') || d.value.contains("rgb(") || d.value.contains("rgba(") {
                return Err(ara_ui_document::Diagnostic::new(
                    7018,
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
