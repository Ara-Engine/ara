//! Project defaults come from the current transactional document, including unsaved ops.
use ara_doc::Workspace;
use ara_play::RenderSettings;

pub(crate) fn read(workspace: &Workspace) -> Result<RenderSettings, String> {
    workspace
        .doc("pj_project")
        .and_then(|doc| doc.record("p_settings"))
        .and_then(|record| record.get("settings"))
        .and_then(|settings| settings.get("rendering"))
        .map_or_else(
            || Ok(RenderSettings::default()),
            |value| {
                serde_json::from_value(value.to_owned())
                    .map_err(|error| format!("project rendering settings: {error}"))
            },
        )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn uses_current_project_settings_and_rejects_unknown_presets() {
        let mut workspace = Workspace::default();
        assert_eq!(read(&workspace).unwrap(), RenderSettings::default());
        let mut project =
            ara_doc::Document::new("project", ara_doc::AssetId::parse("pj_project").unwrap());
        project.insert_unchecked(
            ara_doc::RecordId::parse("p_settings").unwrap(),
            serde_json::from_value(
                serde_json::json!({"settings":{"rendering":{"quality":"medium"}}}),
            )
            .unwrap(),
        );
        workspace.insert("ara.project.json", project);
        assert_eq!(
            read(&workspace).unwrap().quality,
            ara_play::QualitySetting::Medium
        );
        let mut project = workspace.doc("pj_project").unwrap().clone();
        project.insert_unchecked(
            ara_doc::RecordId::parse("p_settings").unwrap(),
            serde_json::from_value(
                serde_json::json!({"settings":{"rendering":{"quality":"typo"}}}),
            )
            .unwrap(),
        );
        workspace.insert("ara.project.json", project);
        assert!(read(&workspace).is_err());
    }
}
