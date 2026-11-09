//! Compatible selected importers share controls; their documents remain independent.
mod commands;
mod defaults;
#[cfg(test)]
mod tests;
mod view;
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Source {
    doc: String,
    source: String,
}
#[derive(Clone)]
struct Group {
    importer: String,
    sources: Vec<Source>,
    project: bool,
}

/// Retained compatible cohorts; control targets capture the exact selected documents.
#[derive(Default)]
pub(crate) struct State {
    groups: BTreeMap<String, Group>,
    filter: Option<String>,
    defaults_open: bool,
    default_type: String,
}

/// One edited override; lowering merges with each latest independent sidecar.
pub(crate) enum Change {
    Setting {
        doc: String,
        field: String,
        value: Value,
    },
    Preset {
        doc: String,
        preset: Option<ara_asset::CookPreset>,
    },
    ProjectField {
        importer: String,
        field: String,
        value: Value,
    },
    ProjectPreset(ara_asset::CookPreset),
    Reset(String),
}

impl Shell {
    /// Native bool/numeric/Select actions commit once; text retains an explicit draft.
    pub(crate) fn import_batch_value(
        &mut self,
        editor: &mut Editor,
        group: &str,
        key: &str,
        value: Value,
    ) -> ara_ui::Result<()> {
        let changes = self.import_batch_changes(group, key, value)?;
        commands::apply(editor, changes, "Edit selected importer settings")?;
        Ok(())
    }
    pub(super) fn import_batch_changes(
        &self,
        group: &str,
        key: &str,
        value: Value,
    ) -> ara_ui::Result<Vec<Change>> {
        if group == "@project" {
            return Ok(vec![Change::ProjectPreset(serde_json::from_value(value)?)]);
        }
        if let Some(doc) = group.strip_prefix("@source:") {
            return Ok(vec![if key == "$preset" {
                Change::Preset {
                    doc: doc.into(),
                    preset: if value.is_null() {
                        None
                    } else {
                        Some(serde_json::from_value(value)?)
                    },
                }
            } else {
                Change::Setting {
                    doc: doc.into(),
                    field: key.into(),
                    value,
                }
            }]);
        }
        if let Some(importer) = group.strip_prefix("@default:") {
            settings(importer)?;
            return Ok(vec![Change::ProjectField {
                importer: importer.into(),
                field: key.into(),
                value,
            }]);
        }
        let cohort = self
            .import_batch
            .groups
            .get(group)
            .ok_or("The import selection changed; choose the files again")?;
        cohort
            .sources
            .iter()
            .map(|source| {
                if key == "$preset" {
                    Ok(Change::Preset {
                        doc: source.doc.clone(),
                        preset: if value.is_null() {
                            None
                        } else {
                            Some(serde_json::from_value(value.clone())?)
                        },
                    })
                } else {
                    Ok(Change::Setting {
                        doc: source.doc.clone(),
                        field: key.into(),
                        value: value.clone(),
                    })
                }
            })
            .collect()
    }

    /// Draft keys retain a cohort identity, never apply to a later selection.
    pub(crate) fn import_batch_field(
        &mut self,
        editor: &mut Editor,
        group: &str,
        key: &str,
        value: Value,
    ) -> ara_ui::Result<bool> {
        if value.is_boolean() || value.is_number() {
            self.import_batch_value(editor, group, key, value)?;
            return Ok(true);
        }
        let text = value
            .as_str()
            .ok_or("Importer text field emitted a non-text value")?;
        self.drafts
            .insert(format!("@import-batch|{group}|{key}"), text.into());
        Ok(false)
    }

    pub(super) fn choose_import_group(&mut self, value: Value) -> ara_ui::Result<()> {
        let group = value.as_str().ok_or("Choose a compatible importer type")?;
        if self.import_batch.defaults_open {
            settings(group)?;
            self.import_batch.default_type = group.into();
            return Ok(());
        }
        if !self
            .import_batch
            .groups
            .values()
            .any(|candidate| candidate.importer == group)
        {
            return Err("That importer type is no longer selected".into());
        }
        self.import_batch.filter = Some(group.into());
        Ok(())
    }
}

pub(crate) fn apply_changes(
    editor: &mut Editor,
    changes: Vec<Change>,
    why: &str,
) -> ara_ui::Result<bool> {
    commands::apply(editor, changes, why)
}
pub(crate) fn inherited_defaults(
    editor: &Editor,
    importer: &str,
    mut builtin: Value,
) -> ara_ui::Result<Value> {
    let inherited = commands::policy(editor)?.effective_settings(importer, &json!({}))?;
    if let Some(fields) = inherited.as_object() {
        for (key, value) in fields {
            builtin[key] = value.clone();
        }
    }
    Ok(builtin)
}
pub(crate) fn enum_values(options: Vec<String>, nullable: bool) -> Vec<(Value, String)> {
    let mut values = Vec::new();
    if nullable {
        values.push((Value::Null, "Use source/global quality".into()));
    }
    values.extend(
        options
            .into_iter()
            .map(|option| (json!(option), super::inspector_panel::sentence(&option))),
    );
    values
}
pub(super) fn numeric_bounds(
    db: &ara_schema::SchemaDb,
    field: Option<&ara_schema::FieldDesc>,
) -> String {
    if let Some(crate::inspector::Widget::Number { min, max, step, .. }) =
        field.map(|field| crate::inspector::widget(db, field))
    {
        let mut out = format!("step='{step}'");
        if let Some(min) = min {
            out.push_str(&format!(" min='{min}'"));
        }
        if let Some(max) = max {
            out.push_str(&format!(" max='{max}'"));
        }
        out
    } else {
        String::new()
    }
}

fn settings(importer: &str) -> ara_ui::Result<(Value, &'static str)> {
    Ok(match importer {
        "ara.texture" => (
            ara_json::to_value(&ara_importers::TextureSettings::default())?,
            "TextureSettings",
        ),
        "ara.audio" => (
            ara_json::to_value(&ara_importers::audio::Settings::default())?,
            "AudioImportSettings",
        ),
        "ara.gltf" | "ara.fbx" => (
            ara_json::to_value(&ara_importers::MeshSettings::default())?,
            "MeshSettings",
        ),
        _ => return Err(format!("Importer {importer} has no editable shared schema").into()),
    })
}
