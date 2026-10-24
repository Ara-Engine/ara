//! Builds an authored snapshot at an export control boundary without starting players.
use crate::{PlayConfig, PlayService, assets::Assets};
use ara_play::PlayScene;
use std::{collections::BTreeSet, path::Path, time::Duration};

/// Reads the canonical project and waits for its bounded import pipeline, then pins
/// immutable cooked generations. Export callers package these generations and the
/// returned documents; standalone players never discover, cook or import assets.
pub fn package_scene(root: &Path, worker: &Path) -> Result<PlayScene, String> {
    package_scene_with_progress(root, worker, |_| Ok(()))
}

/// Packages a scene while reporting completed-cook counts. Cooking may take any
/// total duration while completions continue. `ARA_EXPORT_COOK_STALL_SECONDS`
/// sets the no-completion timeout (default 600 seconds, positive integers only).
pub fn package_scene_with_progress(
    root: &Path,
    worker: &Path,
    report: impl FnMut(crate::assets::CookProgress) -> Result<(), String>,
) -> Result<PlayScene, String> {
    let stall = std::env::var("ARA_EXPORT_COOK_STALL_SECONDS")
        .map(|value| {
            value
                .parse::<u64>()
                .ok()
                .filter(|seconds| *seconds > 0)
                .ok_or_else(|| {
                    "ARA_EXPORT_COOK_STALL_SECONDS must be a positive integer".to_owned()
                })
        })
        .unwrap_or(Ok(600))?;
    let project = crate::open_project(root).map_err(|error| error.to_string())?;
    let (workspace, diagnostics) = project.load(&[]);
    let problems: Vec<_> = project
        .diags
        .iter()
        .chain(diagnostics.values().flatten())
        .filter(|diagnostic| diagnostic.severity == ara_diag::Severity::Error)
        .map(ToString::to_string)
        .collect();
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let main = project.main_scene().ok_or("project has no main scene")?;
    let scene = workspace
        .docs()
        .find(|(path, doc)| *path == main || doc.id.as_str() == main)
        .map(|(_, doc)| doc.id.to_string())
        .ok_or("project main scene is not loaded")?;
    let mut wanted = BTreeSet::new();
    for (_, doc) in workspace.docs() {
        for record in doc.records().values() {
            crate::collect_refs(&serde_json::Value::Object(record.to_owned()), &mut wanted);
        }
    }
    let mut assets = Assets::new(root, worker)?;
    assets.synchronize(&workspace, 0)?;
    let pins = assets.pins_with_progress(&wanted, Duration::from_secs(stall), report)?;
    for (id, _) in workspace
        .index()
        .imports()
        .filter(|(id, _)| wanted.contains(*id))
    {
        if !pins.contains_key(id) {
            return Err(format!("export requires cooked generation {id}"));
        }
    }
    let mut types = Vec::new();
    crate::types_files(root, root, &mut types);
    let service = PlayService::new(root, PlayConfig::for_project(root));
    let scripts = service.script_scene().map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    Ok(PlayScene {
        format: 1,
        scene,
        documents: PlayScene::documents(&workspace)
            .into_iter()
            .filter(|document| !matches!(document.kind.as_str(), "project" | "import"))
            .collect(),
        types,
        seed: 1,
        tick_hz: 60,
        assets: pins,
        cache: assets.cache.to_string_lossy().into_owned(),
        nav_cache: None,
        rendering: crate::rendering::read(&workspace)?,
        scripts,
        imports: workspace
            .index()
            .imports()
            .map(|(id, path)| (id.to_owned(), path.to_owned()))
            .collect(),
    })
}
