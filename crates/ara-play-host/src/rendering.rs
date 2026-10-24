//! Project defaults come from the current transactional document, including unsaved ops.
use ara_doc::Workspace;
use ara_play::RenderSettings;

/// The pool opens its logical device before a PlayScene is transferred. Read
/// only this small project policy when creating the pool, ahead of GPU warm-up.
pub(crate) fn project(root: &std::path::Path) -> Result<RenderSettings, String> {
    let bytes = match std::fs::read(root.join("ara.project.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RenderSettings::default());
        }
        Err(error) => return Err(error.to_string()),
    };
    let project: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    project.get("rendering").map_or_else(
        || Ok(RenderSettings::default()),
        |settings| {
            serde_json::from_value(settings.to_owned())
                .map_err(|error| format!("project rendering settings: {error}"))
        },
    )
}

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
    fn pool_reads_the_project_rt_policy_before_gpu_warmup() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-play-host/render-policy")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&root).unwrap();
        for enabled in [false, true] {
            std::fs::write(root.join("ara.project.json"),
                ara_json::to_canonical_string(&serde_json::json!({"name":"GPU policy", "rendering":{"quality":"high", "rayTracing":enabled}}))).unwrap();
            assert_eq!(project(&root).unwrap().ray_tracing, enabled);
            assert_eq!(
                project(&root).unwrap().quality,
                ara_play::QualitySetting::High
            );
        }
    }
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
