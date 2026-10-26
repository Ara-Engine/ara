//! Component section preferences and clipboard actions. Presentation order never
//! changes component identity or the canonical order of authoring document keys.
use super::*;
use commands::Target;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Preferences {
    collapsed: BTreeSet<String>,
    order: Vec<String>,
}
pub(super) struct Sections {
    preferences: Preferences,
    components: Vec<String>,
    writer: crate::persist::BackgroundWriter<String>,
}
impl Sections {
    pub(super) fn open(root: &std::path::Path) -> Self {
        let path = root.join(".ara/editor/inspector.json");
        let preferences = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            preferences,
            components: Vec::new(),
            writer: crate::persist::BackgroundWriter::new(
                "inspector-preferences",
                move |text: String| {
                    crate::persist::write_text(&path, &text).map_err(|e| e.to_string())
                },
            ),
        }
    }
    pub(super) fn collapsed(&self, name: &str) -> bool {
        self.preferences.collapsed.contains(name)
    }
    pub(super) fn toggle(&mut self, name: &str) {
        if !self.preferences.collapsed.remove(name) {
            self.preferences.collapsed.insert(name.to_owned());
        }
        self.save();
    }
    pub(super) fn expand(&mut self, name: &str) {
        if self.preferences.collapsed.remove(name) {
            self.save();
        }
    }
    pub(super) fn order(&mut self, components: &mut [String]) {
        components.sort_by_key(|name| {
            self.preferences
                .order
                .iter()
                .position(|n| n == name)
                .unwrap_or(self.preferences.order.len() + usize::from(name != "Transform"))
        });
        self.components = components.to_vec();
    }
    pub(super) fn position(&self, name: &str) -> Option<(usize, usize)> {
        self.components
            .iter()
            .position(|component| component == name)
            .map(|index| (index, self.components.len()))
    }
    fn save(&self) {
        self.writer
            .save(ara_json::to_canonical_string(&json!(self.preferences)));
    }
}
impl Shell {
    /// Reset through reversible property resets (including prefab inheritance), or
    /// paste through schema-validated sets. Transform never needs to be removed.
    pub(super) fn component_values(
        &mut self,
        editor: &mut Editor,
        target: &Target,
        reset: bool,
    ) -> ara_ui::Result<()> {
        let Target::Component(name) = target else {
            return Err("Choose a component".into());
        };
        let db = editor.host.store().schema();
        let ty = db.get(name).ok_or("Component schema not found")?;
        let ara_schema::Shape::Struct { fields, .. } = &ty.shape else {
            return Err("Component must have fields".into());
        };
        let copied = if reset {
            None
        } else {
            Some(
                self.component_clipboard
                    .as_ref()
                    .filter(|(kind, _)| kind == name)
                    .map(|(_, value)| value)
                    .ok_or("Copy values from the same component type first")?,
            )
        };
        let mut calls = Vec::new();
        for desc in fields.iter().filter(|f| !f.editor.read_only) {
            if reset {
                calls.extend(editor.state.selection.iter().map(|entity| json!({"cmd":"editor.property.reset","args":{"entity":entity,"path":format!("{name}.{}",desc.name)}})));
                continue;
            }
            let value = copied
                .and_then(|v| v.get(&desc.name))
                .cloned()
                .unwrap_or_else(|| db.field_default(desc));
            calls.extend(crate::inspector::edit_commands(
                &editor.state.selection,
                &format!("{name}.{}", desc.name),
                &value,
            ));
        }
        if !calls.is_empty() {
            editor.apply(
                calls,
                &format!("{} {name} values", if reset { "Reset" } else { "Paste" }),
                None,
            )?;
        }
        Ok(())
    }
    pub(super) fn copy_component(
        &mut self,
        editor: &Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let Target::Component(name) = target else {
            return Err("Choose a component".into());
        };
        let resolved = self.resolved(editor)?;
        let entity = editor
            .state
            .selection
            .first()
            .and_then(|id| resolved.get(id))
            .ok_or("Select an entity first")?;
        let value = entity
            .record
            .get(name)
            .cloned()
            .ok_or("Component no longer exists")?;
        self.component_clipboard = Some((name.clone(), value));
        Ok(())
    }
    pub(super) fn move_component(
        &mut self,
        editor: &Editor,
        target: &Target,
        down: bool,
    ) -> ara_ui::Result<()> {
        let Target::Component(name) = target else {
            return Err("Choose a component".into());
        };
        let resolved = self.resolved(editor)?;
        let entity = editor
            .state
            .selection
            .first()
            .and_then(|id| resolved.get(id))
            .ok_or("Select an entity first")?;
        let mut components: Vec<_> = entity
            .record
            .keys()
            .filter(|key| ara_doc::is_component_key(key))
            .cloned()
            .collect();
        self.inspector_sections.order(&mut components);
        let index = components
            .iter()
            .position(|n| n == name)
            .ok_or("Component no longer exists")?;
        let next = if down {
            index.checked_add(1).filter(|n| *n < components.len())
        } else {
            index.checked_sub(1)
        };
        if let Some(next) = next {
            components.swap(index, next);
            self.inspector_sections.preferences.order = components;
            self.inspector_sections.save();
        }
        Ok(())
    }
}
