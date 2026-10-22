//! Deliberately small CSS grammar with eager property validation and source positions.
use crate::source::{Element, Result};
use std::collections::BTreeMap;
use taffy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);
impl Color {
    fn parse(value: &str) -> Result<Self> {
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
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct State {
    pub hover: bool,
    pub focus: bool,
    pub disabled: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Computed {
    pub layout: Style,
    pub background: Color,
    pub color: Color,
    pub border_color: Color,
    pub radius: f32,
    pub border: f32,
    pub font_size: f32,
    pub clip: bool,
    pub translate: [f32; 2],
}
impl Computed {
    fn defaults(tag: &str) -> Self {
        let control = matches!(tag, "button" | "field");
        let mut layout = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            ..Style::default()
        };
        if control {
            layout.padding = Rect::length(10.0);
        }
        Self {
            layout,
            background: if control {
                Color(39, 48, 64, 255)
            } else {
                Color(0, 0, 0, 0)
            },
            color: Color(225, 232, 241, 255),
            border_color: Color(77, 92, 111, 255),
            radius: if control { 6.0 } else { 0.0 },
            border: 0.0,
            font_size: 17.0,
            clip: false,
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
        if state.is_some_and(|s| !matches!(s, "hover" | "focus" | "disabled")) {
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
        if kind == 't'
            && !matches!(
                name,
                "panel" | "text" | "button" | "field" | "image" | "progress"
            )
        {
            return Err(format!("unknown element selector {name}").into());
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
            Some("hover") => s.hover,
            Some("focus") => s.focus,
            Some("disabled") => s.disabled,
            _ => true,
        }
    }
}
pub struct Rule {
    selector: Selector,
    declarations: Vec<(String, String)>,
    body_offset: usize,
}
pub struct Sheet {
    rules: Vec<Rule>,
    tokens: BTreeMap<String, String>,
}
fn scalar(value: &str) -> Result<f32> {
    let n: f32 = value.strip_suffix("px").unwrap_or(value).parse()?;
    if !n.is_finite() || n < 0.0 {
        return Err("length must be finite and nonnegative".into());
    }
    Ok(n)
}
fn position(source: &str, byte: usize) -> String {
    let prefix = &source[..byte];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    format!("line {line}, column {column}")
}
fn dimension(v: &str) -> Result<Dimension> {
    Ok(if v == "auto" {
        Dimension::auto()
    } else if let Some(p) = v.strip_suffix('%') {
        Dimension::percent(scalar(p)? / 100.0)
    } else {
        Dimension::length(scalar(v)?)
    })
}
fn declarations(body: &str) -> Result<Vec<(String, String)>> {
    body.split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let (k, v) = s.split_once(':').ok_or("expected property: value")?;
            Ok((k.trim().into(), v.trim().into()))
        })
        .collect()
}
fn apply(c: &mut Computed, k: &str, v: &str) -> Result<()> {
    match k {
        "background" => c.background = Color::parse(v)?,
        "color" => c.color = Color::parse(v)?,
        "border-color" => c.border_color = Color::parse(v)?,
        "font-size" => c.font_size = scalar(v)?,
        "border-radius" => c.radius = scalar(v)?,
        "border-width" => {
            c.border = scalar(v)?;
            c.layout.border = Rect::length(c.border);
        }
        "width" => c.layout.size.width = dimension(v)?,
        "height" => c.layout.size.height = dimension(v)?,
        "padding" => c.layout.padding = Rect::length(scalar(v)?),
        "gap" => c.layout.gap = Size::length(scalar(v)?),
        "flex-grow" => c.layout.flex_grow = scalar(v)?,
        "display" => {
            c.layout.display = match v {
                "flex" => Display::Flex,
                "grid" => Display::Grid,
                _ => return Err("display must be flex or grid".into()),
            }
        }
        "flex-direction" => {
            c.layout.flex_direction = match v {
                "row" => FlexDirection::Row,
                "column" => FlexDirection::Column,
                _ => return Err("flex-direction must be row or column".into()),
            }
        }
        "grid-template-columns" => {
            c.layout.grid_template_columns = v
                .split_whitespace()
                .map(|s| {
                    s.strip_suffix("fr")
                        .ok_or_else(|| "grid tracks must use fr".into())
                        .and_then(|n| Ok(GridTemplateComponent::Single(fr(scalar(n)?))))
                })
                .collect::<Result<_>>()?;
            if c.layout.grid_template_columns.is_empty() {
                return Err("grid needs tracks".into());
            }
        }
        "overflow" => {
            c.clip = match v {
                "hidden" => true,
                "visible" => false,
                _ => return Err("overflow must be hidden or visible".into()),
            };
        }
        "translate-x" | "translate-y" => {
            let value: f32 = v.strip_suffix("px").unwrap_or(v).parse()?;
            if !value.is_finite() {
                return Err("translation must be finite".into());
            }
            c.translate[usize::from(k == "translate-y")] = value;
        }
        _ => return Err(format!("unknown property {k}").into()),
    }
    Ok(())
}
impl Sheet {
    pub fn parse(source: &str) -> Result<Self> {
        let mut sheet = Self {
            rules: Vec::new(),
            tokens: BTreeMap::new(),
        };
        let mut offset = 0;
        while !source[offset..].trim().is_empty() {
            let open = source[offset..]
                .find('{')
                .map(|n| n + offset)
                .ok_or("CSS001: expected '{'")?;
            let close = source[open..]
                .find('}')
                .map(|n| n + open)
                .ok_or("CSS001: expected '}'")?;
            let selector = source[offset..open].trim();
            let selector_offset =
                offset + source[offset..open].len() - source[offset..open].trim_start().len();
            let decl = declarations(&source[open + 1..close])?;
            let parsed = if selector == ":root" {
                for (k, v) in decl {
                    if !k.starts_with("--") {
                        return Err("CSS002: :root contains tokens only".into());
                    }
                    sheet.tokens.insert(k, v);
                }
                None
            } else {
                Some(Rule {
                    selector: Selector::parse(selector).map_err(|e| {
                        format!(
                            "CSS003 at byte {selector_offset} ({}): {e}",
                            position(source, selector_offset)
                        )
                    })?,
                    declarations: decl,
                    body_offset: open + 1,
                })
            };
            if let Some(rule) = parsed {
                sheet.rules.push(rule);
            }
            offset = close + 1;
        }
        for rule in &sheet.rules {
            for (k, v) in &rule.declarations {
                let byte =
                    rule.body_offset + source[rule.body_offset..].find(k).unwrap_or_default();
                sheet
                    .resolve(v)
                    .and_then(|v| apply(&mut Computed::defaults("panel"), k, &v))
                    .map_err(|e| {
                        format!(
                            "CSS004 at byte {byte} ({}) in selector {} property {k}: {e}",
                            position(source, byte),
                            rule.selector.name
                        )
                    })?;
            }
        }
        sheet.rules.sort_by_key(|r| r.selector.priority());
        Ok(sheet)
    }
    fn resolve(&self, v: &str) -> Result<String> {
        let mut value = v;
        for _ in 0..16 {
            if let Some(token) = value.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
                value = self
                    .tokens
                    .get(token)
                    .ok_or_else(|| format!("CSS005: unknown token {token}"))?;
            } else {
                return Ok(value.into());
            }
        }
        Err("CSS006: cyclic or excessive token nesting".into())
    }
    pub fn compute(&self, e: &Element, state: State) -> Result<Computed> {
        let mut c = Computed::defaults(&e.tag);
        for rule in &self.rules {
            if rule.selector.matches(e, state) {
                for (k, v) in &rule.declarations {
                    apply(&mut c, k, &self.resolve(v)?)?;
                }
            }
        }
        if let Some(local) = e.attrs.get("style") {
            for (k, v) in declarations(local)? {
                self.resolve(&v)
                    .and_then(|v| apply(&mut c, &k, &v))
                    .map_err(|error| {
                        format!("CSS007 node #{} local property {k}: {error}", e.id)
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
