//! Read-only access to optional project policy, independent of project creation.
use ara_doc::Workspace;
use serde_json::Value;

/// Optional canonical policy document; reading support never creates this file.
pub const PREFERENCES: &str = "settings/project.json";
/// Synthetic workspace identity for the optional policy document.
pub const PREFERENCES_ID: &str = "pj_preferences";
/// Legacy starter scene, retained as the conventional read fallback.
pub const MAIN_SCENE: &str = "levels/main.scene";

/// Read one policy key without creating or migrating project files. The optional
/// dedicated document overrides the legacy marker per key, including explicit null.
pub fn read(root: &std::path::Path, key: &str) -> Result<Option<Value>, ara_diag::Diagnostic> {
    let marker = read_object(&root.join(crate::project::MARKER))?;
    let preferences = read_object(&root.join(PREFERENCES))?;
    Ok(preferences.get(key).or_else(|| marker.get(key)).cloned())
}

fn read_object(
    path: &std::path::Path,
) -> Result<serde_json::Map<String, Value>, ara_diag::Diagnostic> {
    use ara_diag::{Diagnostic, codes};
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => {
            return Err(
                Diagnostic::error(codes::IO, error.to_string()).file(path.display().to_string())
            );
        }
    };
    let value = ara_json::parse(&text).map_err(|error| {
        Diagnostic::error(codes::JSON_SYNTAX, error.message)
            .file(path.display().to_string())
            .line_col(error.line, error.column)
    })?;
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(Diagnostic::error(
            codes::INVALID_PARAMS,
            "project policy must be a JSON object",
        )
        .file(path.display().to_string())),
    }
}

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

    fn scratch() -> std::path::PathBuf {
        let root = std::env::var_os("ARA_TEST_ROOT")
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    std::path::PathBuf::from("D:/AraBuild/ux-hub-evidence")
                } else {
                    std::env::temp_dir()
                }
            })
            .join("project-policy-read-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root).expect("scratch");
        root
    }

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

    #[test]
    fn disk_policy_prefers_each_saved_key_and_retains_null_without_creating_support_files() {
        let root = scratch();
        assert_eq!(read(&root, "ui").expect("missing optional files"), None);
        assert!(!root.join(PREFERENCES).exists());
        std::fs::write(
            root.join(crate::project::MARKER),
            r#"{"ui":{"main":"ui/legacy.ui"},"rendering":{"quality":"high"}}"#,
        )
        .expect("marker");
        std::fs::create_dir(root.join("settings")).expect("settings fixture");
        std::fs::write(root.join(PREFERENCES), r#"{"ui":null}"#).expect("saved policy");
        assert_eq!(
            read(&root, "ui").expect("explicit override"),
            Some(Value::Null)
        );
        assert_eq!(
            read(&root, "rendering").expect("per-key fallback"),
            Some(json!({"quality":"high"}))
        );
        assert_eq!(
            read(&root, "physics").expect("omitted engine default"),
            None
        );
        assert!(!root.join(".ara").exists());
        assert!(!root.join(ara_doc::AUTHORING_ROOT).exists());
    }

    #[test]
    fn disk_policy_diagnoses_malformed_and_nonobject_documents() {
        let root = scratch();
        std::fs::write(root.join(crate::project::MARKER), "{").expect("broken marker fixture");
        assert_eq!(
            read(&root, "ui").expect_err("syntax diagnostic").code,
            ara_diag::codes::JSON_SYNTAX
        );
        std::fs::write(root.join(crate::project::MARKER), "[]").expect("nonobject marker fixture");
        assert_eq!(
            read(&root, "ui").expect_err("object diagnostic").code,
            ara_diag::codes::INVALID_PARAMS
        );
        std::fs::write(root.join(crate::project::MARKER), "{}").expect("valid marker");
        std::fs::create_dir(root.join("settings")).expect("settings fixture");
        for (text, code) in [
            ("{", ara_diag::codes::JSON_SYNTAX),
            ("null", ara_diag::codes::INVALID_PARAMS),
        ] {
            std::fs::write(root.join(PREFERENCES), text).expect("invalid policy fixture");
            assert_eq!(read(&root, "ui").expect_err("policy diagnostic").code, code);
        }
        assert!(!root.join(".ara").exists());
    }
}
