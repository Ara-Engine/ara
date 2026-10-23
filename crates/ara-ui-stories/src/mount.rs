use crate::{Locale, Result, Story, StoryView, Theme, localize};
use ara_schema::{SchemaDb, Validator};
use ara_ui::{
    Ui,
    source::{Command, DocumentKind, SourceDocument},
    theme::{EDITOR_CSS, EditorTheme},
};
use serde_json::Value;
use std::collections::BTreeSet;

impl Story {
    /// Validate dimensions, unique locales, schema-typed fixture values and deterministic tracks.
    pub fn validate(&self, db: &SchemaDb) -> Result<()> {
        if self.name.is_empty() || self.component.is_empty() || self.views.is_empty() {
            return Err("story needs a name, component and at least one view".into());
        }
        if self.component.starts_with('@') {
            let mut diagnostics = Vec::new();
            Validator::new(db, &mut diagnostics).value(
                &ara_schema::ValueType::Asset(None),
                None,
                &Value::String(self.component.clone()),
                "component",
            );
            if !diagnostics.is_empty() {
                return Err(format!("{diagnostics:?}").into());
            }
        }
        let model = db
            .get(&self.fixture_type)
            .ok_or_else(|| format!("unregistered fixture type {}", self.fixture_type))?;
        let mut diagnostics = Vec::new();
        Validator::new(db, &mut diagnostics).fields(
            model,
            &self.fixture.clone().into_iter().collect(),
            "fixture",
        );
        if !diagnostics.is_empty() {
            return Err(format!("{diagnostics:?}").into());
        }
        let mut locales = BTreeSet::new();
        for locale in &self.locales {
            if locale.id.is_empty() || !locales.insert(&locale.id) {
                return Err("locale identifiers must be nonempty and unique".into());
            }
            self.localized_markup(locale)?;
        }
        let elements = ara_ui::source::compile(&self.markup)?;
        for e in &elements {
            if let Some(key) = e.attrs.get("bind") {
                let field = model
                    .field(key)
                    .ok_or_else(|| format!("unknown fixture binding {key}"))?;
                let value = self
                    .fixture
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| db.field_default(field));
                match e.tag.as_str() {
                    "field" | "input" | "text" | "button" if value.is_string() => {}
                    "toggle" if value.is_boolean() => {}
                    "slider" | "progress" | "splitter" if value.is_number() => {}
                    _ => {
                        let message = format!("binding {key} has the wrong type for {}", e.tag);
                        return Err(message.into());
                    }
                }
            }
        }
        for view in &self.views {
            view.physical_size()?;
            if !locales.contains(&view.locale) {
                return Err(format!("missing locale {}", view.locale).into());
            }
        }
        let mut animated = BTreeSet::new();
        for track in &self.transitions {
            if !animated.insert(&track.node)
                || !elements.iter().any(|e| e.id == track.node)
                || track.from.iter().chain(&track.to).any(|v| !v.is_finite())
            {
                return Err(
                    "transition needs a unique existing node and finite coordinates".into(),
                );
            }
        }
        let mut images = BTreeSet::new();
        for image in &self.images {
            let bytes = image
                .width
                .checked_mul(image.height)
                .and_then(|n| n.checked_mul(4));
            if image.width == 0
                || image.height == 0
                || bytes.map(|n| n as usize) != Some(image.rgba.len())
                || !images.insert(&image.source)
            {
                return Err("invalid or duplicate story image".into());
            }
        }
        ara_ui::theme::check_editor(&self.markup, &self.stylesheet)?;
        Ok(())
    }
    /// Resolve the requested locale without consulting the host's language or environment.
    pub fn locale(&self, id: &str) -> Result<&Locale> {
        self.locales
            .iter()
            .find(|locale| locale.id == id)
            .ok_or_else(|| format!("missing locale {id}").into())
    }
    fn localized_markup(&self, locale: &Locale) -> Result<String> {
        localize_markup(&self.markup, locale)
    }
    /// Mount the actual retained runtime with a validated, typed fixture snapshot and Ara theme.
    pub fn mount(&self, view: &StoryView, db: &SchemaDb) -> Result<Ui> {
        self.validate(db)?;
        view.physical_size()?;
        let markup = self.localized_markup(self.locale(&view.locale)?)?;
        let css = format!("{EDITOR_CSS}\n{}", self.stylesheet);
        let mut ui = Ui::with_tokens(
            &markup,
            &css,
            [view.width as f32, view.height as f32],
            view.theme.native().tokens()?,
        )?;
        let model = db
            .get(&self.fixture_type)
            .ok_or("fixture schema disappeared")?;
        for i in 0..ui.nodes.len() {
            if let Some(key) = ui.nodes[i].element.attrs.get("bind") {
                let field = model
                    .field(key)
                    .ok_or("validated fixture binding disappeared")?;
                let value = self
                    .fixture
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| db.field_default(field));
                match value {
                    Value::String(s) => ui.set_text(i, &s)?,
                    Value::Bool(b) => ui.set_checked(i, b)?,
                    Value::Number(n) => {
                        ui.set_value(i, n.as_f64().ok_or("invalid fixture number")?)?
                    }
                    _ => return Err("unsupported fixture binding type".into()),
                }
            }
        }
        Ok(ui)
    }
}

/// Resolve captions through reversible edits on a temporary source document, before template expansion.
pub fn localize_markup(markup: &str, locale: &Locale) -> Result<String> {
    let mut source = SourceDocument::parse(markup, DocumentKind::Markup)?;
    for e in ara_ui::source::parse(markup)? {
        for attr in ["text", "label", "description"] {
            if let Some(text) = e.attrs.get(attr) {
                // Bound model values are data, not translation keys.
                if attr == "text"
                    && e.attrs.contains_key("bind")
                    && matches!(e.tag.as_str(), "field" | "input" | "text" | "button")
                {
                    continue;
                }
                let value = localize(text, locale)?;
                let op = source.command(
                    source.revision(),
                    Command::SetAttribute {
                        id: e.id.clone(),
                        name: attr.into(),
                        value: Some(value),
                    },
                )?;
                source.apply(&op)?;
            }
        }
    }
    Ok(source.serialize().into())
}
impl StoryView {
    /// Compute physical PNG dimensions with bounded allocation and a 96-DPI logical baseline.
    pub fn physical_size(&self) -> Result<[u32; 2]> {
        if self.width == 0 || self.height == 0 || !(24..=768).contains(&self.dpi) {
            return Err("story view needs positive dimensions and DPI in 24..=768".into());
        }
        let scale = f64::from(self.dpi) / 96.0;
        let size = [self.width, self.height].map(|v| (f64::from(v) * scale).round() as u32);
        if size.contains(&0)
            || size.iter().any(|v| *v > 16384)
            || u64::from(size[0]) * u64::from(size[1]) > 16_777_216
        {
            return Err("capture exceeds 16 megapixels or 16384 pixels per side".into());
        }
        Ok(size)
    }
}
impl Theme {
    /// Map the serialized theme to the production native palette.
    pub fn native(self) -> EditorTheme {
        match self {
            Self::Dark => EditorTheme::Dark,
            Self::Light => EditorTheme::Light,
        }
    }
}
