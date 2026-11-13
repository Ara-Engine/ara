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
    package(root, worker, None, report)
}

/// Packages an export with its explicit precompiled host selection. Unlike Play,
/// this never discovers an author-machine JIT host or compiler server.
pub fn package_scene_for_export(
    root: &Path,
    worker: &Path,
    scripts: Option<ara_play::ScriptScene>,
    report: impl FnMut(crate::assets::CookProgress) -> Result<(), String>,
) -> Result<PlayScene, String> {
    package(root, worker, Some(scripts), report)
}

/// The errors that stop packaging. A project-level error (the C# tools are not built, a
/// module does not compile) leaves the schema without the project's own types, so every
/// document then reports unknown components: the project's errors are returned alone, and
/// the documents' only when the project itself is sound.
fn problems<'a>(
    project: &[ara_diag::Diagnostic],
    documents: impl Iterator<Item = &'a ara_diag::Diagnostic>,
) -> Vec<String> {
    let errors = |diagnostics: &mut dyn Iterator<Item = &ara_diag::Diagnostic>| -> Vec<String> {
        diagnostics
            .filter(|diagnostic| diagnostic.severity == ara_diag::Severity::Error)
            .map(ToString::to_string)
            .collect()
    };
    let project = errors(&mut project.iter());
    if project.is_empty() {
        errors(&mut { documents })
    } else {
        project
    }
}

fn package(
    root: &Path,
    worker: &Path,
    scripts: Option<Option<ara_play::ScriptScene>>,
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
    let problems = problems(&project.diags, diagnostics.values().flatten());
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let main = project.main_scene().ok_or("project has no main scene")?;
    let scene = workspace
        .find(&main)
        .map(ToString::to_string)
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
            // Name the importer's own error, so the export report alone explains a
            // failed cook.
            return Err(match assets.failure(id) {
                Some(error) => {
                    format!("export requires cooked generation {id}: its import failed: {error}")
                }
                None => format!("export requires cooked generation {id}"),
            });
        }
    }
    let mut types = Vec::new();
    crate::types_files(root, root, &mut types);
    let inventory = ara_script::project::inventory(root).map_err(|d| d.to_string())?;
    types.extend(
        inventory
            .modules
            .into_iter()
            .filter(|m| m.profile == ara_script::ModuleProfile::Gameplay)
            .filter_map(|m| m.schema),
    );
    let scripts = if let Some(scripts) = scripts {
        scripts
    } else {
        let service = PlayService::new(root, PlayConfig::for_project(root));
        service.script_scene().map_err(|diagnostics| {
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })?
    };
    Ok(PlayScene {
        ui: crate::game_ui::read(root)?,
        format: 1,
        scene,
        documents: PlayScene::documents(&workspace)
            .into_iter()
            .filter(|document| {
                !workspace.is_project_document(&document.id)
                    && !matches!(document.kind.as_str(), "project" | "import")
            })
            .map(|mut document| {
                if let Some(storage) = workspace.path_of(&document.id) {
                    document.path = workspace.reference_path(storage).to_owned();
                }
                document
            })
            .collect(),
        types,
        seed: 1,
        tick_hz: 60,
        assets: pins,
        cache: assets.cache.to_string_lossy().into_owned(),
        nav_cache: None,
        rendering: crate::rendering::read(&workspace)?,
        render_debug: ara_play::RenderDebug::default(),
        scripts,
        imports: workspace
            .index()
            .imports()
            .map(|(id, path)| (id.to_owned(), path.to_owned()))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::problems;
    use ara_diag::{Code, Diagnostic};

    #[test]
    fn a_project_error_is_reported_without_the_document_errors_it_causes() {
        let tools = Diagnostic::error(Code(904), "C# SDK tools are missing");
        let unknown = Diagnostic::error(Code(1106), "unknown component `Survivor`");
        let warning = Diagnostic::warning(Code(1106), "a warning");
        let both = problems(std::slice::from_ref(&tools), [&unknown].into_iter());
        assert_eq!(both.len(), 1);
        assert!(both[0].contains("ARA0904"), "{both:?}");
        // A sound project reports its documents; warnings never stop packaging.
        let documents = problems(
            std::slice::from_ref(&warning),
            [&unknown, &warning].into_iter(),
        );
        assert_eq!(documents.len(), 1);
        assert!(documents[0].contains("ARA1106"), "{documents:?}");
    }
}
