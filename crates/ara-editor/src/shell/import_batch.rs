//! Compatible selected importers share controls; their documents remain independent.
mod defaults;
#[cfg(test)]
mod tests;
mod view;
use super::*;
use crate::import_edit as commands;
use crate::import_edit::settings;

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

pub(crate) use crate::import_edit::Change;

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
        commands::apply_changes(editor, changes, "Edit selected importer settings")?;
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

pub(crate) use crate::import_edit::{apply_changes, enum_values, inherited_defaults};

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

