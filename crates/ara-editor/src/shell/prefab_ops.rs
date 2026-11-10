//! Prefab workflows lower to one typed cross-document transaction per human action.
use super::commands::Target;
use super::*;

pub use crate::prefab::{Instance, instance, source_choices};

impl Shell {
    /// Preserve context identity and field intent when a source submenu closes.
    pub(crate) fn apply_to_target(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Choice {
            entity: String,
            path: Option<String>,
            destination: String,
        }
        let Target::Text(text) = target else {
            return Err("Choose a prefab source destination".into());
        };
        let choice = if text.starts_with('@') {
            Choice {
                entity: editor
                    .state
                    .selection
                    .first()
                    .ok_or("Select a prefab instance")?
                    .clone(),
                path: None,
                destination: text.clone(),
            }
        } else {
            serde_json::from_str::<Choice>(text)?
        };
        self.apply_overrides_to(
            editor,
            &choice.entity,
            choice.path.as_deref(),
            Some(&choice.destination),
        )
    }

    /// Create/populate the asset and replace its source as one undoable transaction.
    /// Existing instances create inherited variants, preserving their base reference.
    pub(crate) fn prefab_from_selection(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let root = target
            .entity(&editor.state)
            .ok_or("Select an entity")?
            .to_owned();
        let path = crate::prefab::from_selection(editor, &self.browser.files, &root)?;
        self.status_note = Some(format!("Created {path}"));
        Ok(())
    }

    /// Apply to the nearest owning prefab; source and instance change together.
    pub(crate) fn apply_overrides(
        &mut self,
        editor: &mut Editor,
        entity: &str,
        path: Option<&str>,
    ) -> ara_ui::Result<()> {
        self.apply_overrides_to(editor, entity, path, None)
    }

    /// Apply to an explicit nested/variant source chosen from source_choices.
    pub(crate) fn apply_overrides_to(
        &mut self,
        editor: &mut Editor,
        entity: &str,
        path: Option<&str>,
        destination: Option<&str>,
    ) -> ara_ui::Result<()> {
        let source = crate::prefab::apply(editor, entity, path, destination)?;
        self.status_note = Some(format!("Applied to {source}"));
        Ok(())
    }
}
