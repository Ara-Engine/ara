//! Deliberately small CSS grammar with eager property validation and source positions.
use crate::{Result, source::Element};
mod properties;
use properties::{apply, declarations};
use std::collections::BTreeMap;
use taffy::prelude::*;
/// Straight-alpha RGBA8 color in the UI sRGB numeric working space.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Color(
    /// Red channel.
    pub u8,
    /// Green channel.
    pub u8,
    /// Blue channel.
    pub u8,
    /// Alpha channel.
    pub u8,
);
impl Color {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        if value == "transparent" {
            return Ok(Self(0, 0, 0, 0));
        }
        let hex = value
            .strip_prefix('#')
            .ok_or("expected #RRGGBB or #RRGGBBAA")?;
        if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
            return Err("expected #RRGGBB or #RRGGBBAA".into());
        }
        Ok(Self(
            u8::from_str_radix(&hex[0..2], 16)?,
            u8::from_str_radix(&hex[2..4], 16)?,
            u8::from_str_radix(&hex[4..6], 16)?,
            if hex.len() == 8 {
                u8::from_str_radix(&hex[6..8], 16)?
            } else {
                255
            },
        ))
    }
}
/// Independent pointer, focus, active and disabled selector states.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub struct State {
    /// Control is pressed or pointer-captured.
    pub active: bool,
    /// Pointer is over this control.
    pub hover: bool,
    /// Keyboard/gamepad focus belongs to this control.
    pub focus: bool,
    /// Control does not accept activation or focus.
    pub disabled: bool,
}
/// Resolved layout and paint properties for one retained node.
#[derive(Clone, Debug, PartialEq)]
pub struct Computed {
    /// Needs text measurement or geometry recomputation.
    pub layout: Style,
    /// Background fill color.
    pub background: Color,
    /// Foreground or glyph tint color.
    pub color: Color,
    /// `color: inherit`: use the parent's computed color (labels follow their button's state).
    pub inherit_color: bool,
    /// Border/focus outline color.
    pub border_color: Color,
    /// Clamped corner radius in logical pixels.
    pub radius: f32,
    /// Inset border thickness in logical pixels.
    pub border: f32,
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Explicit font family list, or the host text system default.
    pub font_family: Option<String>,
    /// Align text to the trailing edge of its content box.
    pub text_end: bool,
    /// Whether text may wrap at the available content width.
    pub wrap_text: bool,
    /// Intersected ancestor clip bounds in viewport coordinates.
    pub clip: bool,
    /// Paint/input translation relative to the layout box.
    pub translate: [f32; 2],
}
impl Computed {
    /// Native primitive defaults before theme and source rules are applied.
    pub fn defaults(tag: &str) -> Self {
        let control = matches!(
            tag,
            "button" | "field" | "input" | "toggle" | "slider" | "dropdown" | "tab"
        );
        // Every node is a containing block: an absolute child's insets (including
        // `right`/`bottom`) resolve against its parent, which is where paint places it.
        // Taffy's own default is `Static` (CSS), which resolves them against the root.
        let mut layout = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            position: Position::Relative,
            ..Style::default()
        };
        if control {
            layout.padding = Rect::length(10.0);
        }
        // Placed by `Ui::frame` (`popover`); a block keeps its rows at natural height
        // when the popover is clamped and scrolls.
        let popover = tag == "popover";
        if popover {
            layout.display = Display::Block;
            layout.position = Position::Absolute;
        }
        Self {
            layout,
            background: if control {
                Color(39, 48, 64, 255)
            } else {
                Color(0, 0, 0, 0)
            },
            color: Color(225, 232, 241, 255),
            inherit_color: false,
            border_color: Color(77, 92, 111, 255),
            radius: if control { 6.0 } else { 0.0 },
            border: 0.0,
            font_size: 17.0,
            font_family: None,
            text_end: false,
            wrap_text: true,
            clip: popover || tag == "scroll",
            translate: [0.0; 2],
        }
    }
}
struct Selector {
    kind: char,
    name: String,
    state: Option<String>,
}
impl Selector {
    fn parse(s: &str) -> Result<Self> {
        let (base, state) = s.split_once(':').map_or((s, None), |(a, b)| (a, Some(b)));
        if state.is_some_and(|s| !matches!(s, "hover" | "focus" | "active" | "disabled")) {
            return Err(format!("unknown state selector {s}").into());
        }
        let (kind, name) = match base.as_bytes().first() {
            Some(b'.') => ('.', &base[1..]),
            Some(b'#') => ('#', &base[1..]),
            _ => ('t', base),
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(format!(
                "unsupported selector {s:?}; use type, .class, #id, optionally one state"
            )
            .into());
        }
        if kind == 't' && !ara_ui_document::stylesheet::TYPES.contains(&name) {
            return Err(format!("ARA4010: unknown element selector {name}").into());
        }
        Ok(Self {
            kind,
            name: name.into(),
            state: state.map(str::to_owned),
        })
    }
    fn priority(&self) -> u16 {
        (if self.kind == '#' {
            100
        } else if self.kind == '.' {
            10
        } else {
            1
        }) + if self.state.is_some() { 10 } else { 0 }
    }
    fn matches(&self, e: &Element, s: State) -> bool {
        let base = match self.kind {
            '#' => e.id == self.name,
            '.' => e
                .attrs
                .get("class")
                .is_some_and(|v| v.split_whitespace().any(|c| c == self.name)),
            _ => e.tag == self.name,
        };
        base && match self.state.as_deref() {
            Some("active") => s.active,
            Some("hover") => s.hover,
            Some("focus") => s.focus,
            Some("disabled") => s.disabled,
            _ => true,
        }
    }
}
/// Compiled selector and validated native declarations.
pub struct Rule {
    selector: Selector,
    source_selector: String,
    declarations: Vec<ara_ui_document::stylesheet::Declaration>,
    body_offset: usize,
}
/// Typed stylesheet rules and semantic tokens in cascade order. Computed styles are
/// memoized by what selectors can see (tag, class, inline style, state and, when an
/// ID rule names it, the ID); swapping tokens (`set_tokens`) clears the memo.
pub struct Sheet {
    rules: Vec<Rule>,
    tokens: BTreeMap<String, String>,
    /// `:root` tokens from the source, under any theme overrides.
    base_tokens: BTreeMap<String, String>,
    /// The theme's overrides on top of `base_tokens`.
    overrides: BTreeMap<String, String>,
    /// IDs some rule selects by `#id`.
    id_rules: std::collections::HashSet<String>,
    memo: std::cell::RefCell<std::collections::HashMap<StyleKey, Computed>>,
}
/// What a computed style depends on, apart from the sheet itself.
#[derive(PartialEq, Eq, Hash)]
struct StyleKey {
    tag: String,
    class: Option<String>,
    style: Option<String>,
    id: Option<String>,
    state: State,
}
impl Sheet {
    /// Eagerly validate local declarations with an ARA location in the original markup.
    pub fn validate_inline(&self, markup: &str) -> Result<()> {
        for block in ara_ui_document::inline_styles(markup)? {
            let validate = || -> Result<()> {
                for (property, value) in declarations(&block.value)? {
                    apply(
                        &mut Computed::defaults("panel"),
                        &property,
                        &self.resolve(&value)?,
                    )?;
                }
                Ok(())
            };
            validate().map_err(|e| {
                ara_ui_document::Diagnostic::new(
                    ara_ui_document::codes::INLINE_STYLE,
                    markup,
                    block.bytes,
                    format!("Node {} inline style: {e}", block.id),
                    "Use a supported native declaration with valid units and semantic tokens",
                )
            })?;
        }
        Ok(())
    }
    /// Parse and eagerly validate all native syntax before creating runtime state.
    pub fn parse(source: &str) -> Result<Self> {
        Self::with_tokens(source, BTreeMap::new())
    }
    /// Compile a stylesheet using explicit semantic theme overrides.
    pub fn with_tokens(source: &str, tokens: BTreeMap<String, String>) -> Result<Self> {
        let syntax = ara_ui_document::stylesheet::Stylesheet::parse(source)?;
        let mut sheet = Self {
            rules: Vec::new(),
            tokens: syntax.tokens.clone(),
            base_tokens: syntax.tokens,
            overrides: tokens.clone(),
            id_rules: Default::default(),
            memo: Default::default(),
        };
        sheet.tokens.extend(tokens);
        for rule in syntax.rules {
            if rule.selector == ":root" {
                continue;
            }
            sheet.rules.push(Rule {
                selector: Selector::parse(&rule.selector)?,
                source_selector: rule.selector,
                declarations: rule.declarations,
                body_offset: rule.bytes.start,
            });
        }
        for rule in &sheet.rules {
            for d in &rule.declarations {
                sheet
                    .resolve(&d.value)
                    .and_then(|v| apply(&mut Computed::defaults("panel"), &d.name, &v))
                    .map_err(|error| {
                        ara_ui_document::Diagnostic::new(
                            ara_ui_document::codes::PROPERTY,
                            source,
                            d.bytes.clone(),
                            format!(
                                "Selector {} property {}: {error}",
                                rule.selector.name, d.name
                            ),
                            "Use a documented native property value or a defined semantic token",
                        )
                    })?;
            }
        }
        sheet.rules.sort_by_key(|r| r.selector.priority());
        sheet.id_rules = sheet
            .rules
            .iter()
            .filter(|r| r.selector.kind == '#')
            .map(|r| r.selector.name.clone())
            .collect();
        Ok(sheet)
    }
    /// Replace the theme's token overrides without recompiling rules (a theme switch).
    /// Returns whether any token changed; unchanged tokens keep every computed style.
    pub fn set_tokens(&mut self, tokens: BTreeMap<String, String>) -> bool {
        let mut next = self.base_tokens.clone();
        next.extend(tokens.clone());
        self.overrides = tokens;
        if next == self.tokens {
            return false;
        }
        self.tokens = next;
        self.memo.get_mut().clear();
        true
    }
    /// The theme overrides in effect (kept when the stylesheet is replaced).
    pub fn overrides(&self) -> &BTreeMap<String, String> {
        &self.overrides
    }
    /// Matching source declarations in cascade order, for inspector provenance.
    pub fn provenance(&self, e: &Element, state: State) -> Vec<serde_json::Value> {
        let mut values = Vec::new();
        for rule in &self.rules {
            if rule.selector.matches(e, state) {
                for d in &rule.declarations {
                    let (k, v) = (&d.name, &d.value);
                    values.push(serde_json::json!({"selector":rule.source_selector,"property":k,"authored":v,"resolved":self.resolve(v).ok(),"byte":rule.body_offset,"valueBytes":d.bytes}));
                }
            }
        }
        if let Some(local) = e.attrs.get("style") {
            values.push(serde_json::json!({"local":local}));
        }
        values
    }
    /// Resolve a semantic color token without allocating.
    pub fn color_token(&self, name: &str) -> Option<Color> {
        self.tokens.get(name).and_then(|v| Color::parse(v).ok())
    }
    fn resolve(&self, v: &str) -> Result<String> {
        let mut value = v;
        for _ in 0..16 {
            if let Some(token) = value.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
                value = self
                    .tokens
                    .get(token)
                    .ok_or_else(|| format!("ARA4012: unknown token {token}"))?;
            } else {
                return Ok(value.into());
            }
        }
        Err("ARA4013: cyclic or excessive token nesting".into())
    }
    /// Apply matching rules in specificity/source order, followed by local overrides
    /// (memoized: equal tag, class, inline style and state share one cascade).
    pub fn compute(&self, e: &Element, state: State) -> Result<Computed> {
        let key = StyleKey {
            tag: e.tag.clone(),
            class: e.attrs.get("class").cloned(),
            style: e.attrs.get("style").cloned(),
            id: self.id_rules.contains(&e.id).then(|| e.id.clone()),
            state,
        };
        if let Some(c) = self.memo.borrow().get(&key) {
            return Ok(c.clone());
        }
        let c = self.cascade(e, state)?;
        let mut memo = self.memo.borrow_mut();
        // Positional inline styles (tooltips, overlays) are unbounded; start over.
        if memo.len() >= 4096 {
            memo.clear();
        }
        memo.insert(key, c.clone());
        Ok(c)
    }
    fn cascade(&self, e: &Element, state: State) -> Result<Computed> {
        let mut c = Computed::defaults(&e.tag);
        if let Some(value) = self.tokens.get("--foreground") {
            c.color = Color::parse(value)?;
            c.font_size = 13.0;
        }
        if let Some(value) = self.tokens.get("--border") {
            c.border_color = Color::parse(value)?;
        }
        if matches!(
            e.tag.as_str(),
            "button" | "field" | "input" | "toggle" | "slider" | "dropdown" | "tab"
        ) && let Some(value) = self.tokens.get("--secondary")
        {
            c.background = Color::parse(value)?;
        }

        for rule in &self.rules {
            if rule.selector.matches(e, state) {
                for d in &rule.declarations {
                    let (k, v) = (&d.name, &d.value);
                    apply(&mut c, k, &self.resolve(v)?)?;
                }
            }
        }
        if let Some(local) = e.attrs.get("style") {
            for (k, v) in declarations(local)? {
                self.resolve(&v)
                    .and_then(|v| apply(&mut c, &k, &v))
                    .map_err(|error| {
                        format!("ARA4014 node #{} local property {k}: {error}", e.id)
                    })?;
            }
        }
        Ok(c)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_css_is_an_error_even_for_unmatched_rules() {
        for source in [
            "button > text { gap: 4px; }",
            "button:visited { color:#112233; }",
            ".unused { position:fixed; }",
            "text { color: var(--missing); }",
        ] {
            assert!(Sheet::parse(source).is_err(), "{source}");
        }
    }
    #[test]
    fn flex_basis_is_validated_and_drives_equal_weight_dock_sizes() -> Result<()> {
        let sheet = Sheet::parse("panel { flex-basis:0px; flex-grow:1; }")?;
        let elements = crate::source::parse("<panel id='dock' />")?;
        assert_eq!(
            sheet
                .compute(&elements[0], State::default())?
                .layout
                .flex_basis,
            taffy::Dimension::length(0.)
        );
        assert!(Sheet::parse("panel { flex-basis:-1px; }").is_err());
        Ok(())
    }
    #[test]
    fn nowrap_labels_stay_on_one_line_and_focus_can_transfer_to_a_native_view() -> Result<()> {
        let mut ui = crate::Ui::parse(
            "<ui id='root'><field id='field' text='x' /><button id='button' text='Move W' style='width:40px; white-space:nowrap' /></ui>",
            "ui {width:200px; height:100px}",
            [200., 100.],
        )?;
        let mut text = ara_ui_text::TextSystem::bundled()?;
        ui.frame(&mut text)?;
        let i = ui.find("button").expect("authored button");
        assert_eq!(
            ui.nodes[i]
                .text
                .as_ref()
                .expect("text")
                .layout
                .lines()
                .count(),
            1
        );
        let mut input = crate::input::InputState::default();
        input.focus(&mut ui, crate::source::stable_id("field"))?;
        input.blur(&mut ui);
        assert_eq!(input.focused(), None);
        assert!(Sheet::parse("text {white-space:pre}").is_err());
        Ok(())
    }
    #[test]
    fn cascade_tokens_specificity_state_and_local_override() -> Result<()> {
        let sheet = Sheet::parse(
            ":root {--accent:#112233;} button {background:#000000;} .primary {background:var(--accent);} #go:focus {background:#445566;}",
        )?;
        let e = crate::source::parse("<button id='go' class='primary' text='Go' />")?;
        assert_eq!(
            sheet.compute(&e[0], State::default())?.background,
            Color(17, 34, 51, 255)
        );
        assert_eq!(
            sheet
                .compute(
                    &e[0],
                    State {
                        focus: true,
                        ..State::default()
                    }
                )?
                .background,
            Color(68, 85, 102, 255)
        );
        Ok(())
    }
}
