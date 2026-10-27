//! Read-only access to optional project policy, independent of project creation.
use ara_doc::Workspace;
use serde_json::Value;

/// Optional canonical policy document; reading support never creates this file.
pub const PREFERENCES: &str = "settings/project.json";
/// Synthetic workspace identity for the optional policy document.
pub const PREFERENCES_ID: &str = "pj_preferences";
/// Legacy starter scene, retained as the conventional read fallback.
pub const MAIN_SCENE: &str = "levels/main.scene";

/// Borrow current transactional policy, preferring the dedicated document and
/// falling back per key to the legacy marker. Explicit null remains an override.
pub fn policy<'a>(workspace: &'a Workspace, key: &str) -> Option<&'a Value> {
    [PREFERENCES_ID, "pj_project"].into_iter().find_map(|id| {
        workspace
            .doc(id)
            .and_then(|document| document.record("p_settings"))
            .and_then(|record| record.get("settings"))
            .and_then(|settings| settings.get(key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_doc::{AssetId, Document, RecordId};
    use serde_json::json;

    fn settings(id: &str, value: Value) -> Document {
        let mut document = Document::new("project", AssetId::parse(id).expect("project ID"));
        document.insert_unchecked(
            RecordId::parse("p_settings").expect("settings ID"),
            serde_json::Map::from_iter([("settings".into(), value)]),
        );
        document
    }

    #[test]
    fn policy_borrows_current_values_and_falls_back_per_key_without_creating_documents() {
        let mut workspace = Workspace::default();
        workspace.insert_project_document(
            "ara.project.json",
            settings(
                "pj_project",
                json!({"main":"levels/main.scene","ui":{"main":"ui/legacy.ui"}}),
            ),
        );
        assert_eq!(
            policy(&workspace, "main"),
            Some(&json!("levels/main.scene"))
        );
        assert!(workspace.doc(PREFERENCES_ID).is_none());
        workspace.insert_project_document(
            PREFERENCES,
            settings(PREFERENCES_ID, json!({"ui":{"main":"features/hud.ui"}})),
        );
        let saved = &workspace
            .doc(PREFERENCES_ID)
            .expect("policy")
            .record("p_settings")
            .expect("record")["settings"]["ui"];
        assert!(std::ptr::eq(
            policy(&workspace, "ui").expect("current value"),
            saved
        ));
        assert_eq!(
            policy(&workspace, "main"),
            Some(&json!("levels/main.scene"))
        );
    }

    #[test]
    fn explicit_null_policy_does_not_restore_a_legacy_override() {
        let mut workspace = Workspace::default();
        workspace.insert_project_document(
            "ara.project.json",
            settings("pj_project", json!({"ui":{"main":"legacy.ui"}})),
        );
        workspace
            .insert_project_document(PREFERENCES, settings(PREFERENCES_ID, json!({"ui":null})));
        assert_eq!(policy(&workspace, "ui"), Some(&Value::Null));
    }
}
