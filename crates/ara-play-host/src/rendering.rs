//! Project defaults come from the current transactional document, including unsaved ops.
use ara_doc::Workspace;
use ara_play::RenderSettings;

/// The pool opens its logical device before a PlayScene is transferred. Read
/// only this small project policy when creating the pool, ahead of GPU warm-up.
pub(crate) fn project(root: &std::path::Path) -> Result<RenderSettings, String> {
    let settings =
        ara_host::project_policy::read(root, "rendering").map_err(|error| error.to_string())?;
    settings.map_or_else(
        || Ok(RenderSettings::default()),
        |settings| {
            serde_json::from_value(settings)
                .map_err(|error| format!("project rendering settings: {error}"))
        },
    )
}

pub(crate) fn read(workspace: &Workspace) -> Result<RenderSettings, String> {
    ara_host::project_policy::policy(workspace, "rendering").map_or_else(
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

    fn scratch() -> tempfile::TempDir {
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
            .join("render-policy-read-tests");
        std::fs::create_dir_all(&root).expect("scratch root");
        tempfile::Builder::new()
            .prefix("render-policy-")
            .tempdir_in(root)
            .expect("isolated scratch")
    }

    #[test]
    fn pool_reads_the_project_rt_policy_before_gpu_warmup() {
        let temporary = scratch();
        let root = temporary.path();
        for enabled in [false, true] {
            std::fs::write(root.join("ara.project.json"),
                ara_json::to_canonical_string(&serde_json::json!({"name":"GPU policy", "rendering":{"quality":"high", "rayTracing":enabled}}))).unwrap();
            assert_eq!(project(root).unwrap().ray_tracing, enabled);
            assert_eq!(
                project(root).unwrap().quality,
                ara_play::QualitySetting::High
            );
        }
        std::fs::create_dir(root.join("settings")).expect("settings fixture");
        std::fs::write(
            root.join(ara_host::project_policy::PREFERENCES),
            r#"{"rendering":{"quality":"medium","rayTracing":false}}"#,
        )
        .expect("saved policy");
        assert_eq!(
            project(root).expect("saved rendering").quality,
            ara_play::QualitySetting::Medium
        );
        assert!(!project(root).expect("saved RT policy").ray_tracing);
        std::fs::write(root.join(ara_host::project_policy::PREFERENCES), "{}")
            .expect("empty overrides");
        assert!(project(root).expect("legacy fallback").ray_tracing);
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
