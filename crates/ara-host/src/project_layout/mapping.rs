//! Preflights overlapping legacy roots before scheduling any filesystem changes.
use super::migrate::{Migration, Rewrite, collect, io_error, problem};
use ara_diag::Diagnostic;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) fn plan(migration: &mut Migration) -> Result<Vec<(String, String)>, Diagnostic> {
    let mut files = Vec::new();
    collect(&migration.root, &migration.root, &mut files)?;
    let modern = migration.modern;
    let mut modules = Vec::new();
    for source in &files {
        if source
            .file_name()
            .is_some_and(|name| name == "ara.module.toml")
        {
            let text = fs::read_to_string(source).map_err(io_error)?;
            let parsed: toml::Value =
                toml::from_str(&text).map_err(|e| problem(format!("{}: {e}", source.display())))?;
            let module = parsed
                .get("module")
                .ok_or_else(|| problem(format!("{} has no [module] table", source.display())))?;
            let name = module
                .get("name")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| problem("legacy module needs a name"))?;
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
            {
                return Err(problem(format!(
                    "module name `{name}` cannot form a lowercase .module.json filename"
                )));
            }
            let mut value = serde_json::json!({"name":name});
            if let Some(references) = module.get("references") {
                value["references"] =
                    serde_json::to_value(references).map_err(|e| problem(e.to_string()))?;
            }
            if let Some(hot) = module.get("hot") {
                value["hot"] = serde_json::to_value(hot).map_err(|e| problem(e.to_string()))?;
            }
            let editor = module.get("profile").and_then(toml::Value::as_str) == Some("editor");
            modules.push((
                source.parent().expect("manifest has a parent").to_owned(),
                name.to_owned(),
                editor,
                value,
            ));
        }
    }
    modules.sort_by_key(|(path, ..)| std::cmp::Reverse(path.components().count()));
    let mut targets = BTreeMap::<String, PathBuf>::new();
    let mut mappings = Vec::new();
    let mut module_names = std::collections::BTreeSet::new();
    for (directory, name, editor, _) in &modules {
        if !module_names.insert(name.to_ascii_lowercase()) {
            return Err(problem(format!(
                "module name `{name}` is used more than once"
            )));
        }
        let physical = relative_path(&migration.root, directory)?;
        if let Some(relative) = authored_path(&physical, modern)
            && *editor
            && !relative.split('/').any(|part| part == "editor")
        {
            mappings.push((physical, format!("editor/{relative}")));
        }
    }
    for source in files {
        let relative = relative(&migration.root, &source)?;
        let Some(mut authored) = authored_path(&relative, modern) else {
            continue;
        };
        let module = modules
            .iter()
            .find(|(directory, ..)| source.starts_with(directory));
        if let Some((directory, name, editor, _)) = module {
            let normal = authored_path(&relative_path(&migration.root, directory)?, modern)
                .unwrap_or_default();
            if *editor && !normal.split('/').any(|part| part == "editor") {
                authored = format!("editor/{authored}");
            }
            if source.file_name().is_some_and(|n| n == "ara.module.toml") {
                let parent = Path::new(&authored).parent().unwrap_or(Path::new(""));
                authored = parent
                    .join(format!("{}.module.json", name.to_ascii_lowercase()))
                    .to_string_lossy()
                    .replace('\\', "/");
            }
            // Legacy reflection output is generated, never an authored schema.
            let relative = source
                .strip_prefix(directory)
                .expect("matched module contains source");
            let generated = source
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".types.json"))
                || source
                    .extension()
                    .is_some_and(|ext| ext == "csproj" || ext == "sln" || ext == "slnx")
                || relative
                    .components()
                    .next()
                    .is_some_and(|part| part.as_os_str() == "obj" || part.as_os_str() == "bin");
            if generated {
                let target = migration
                    .root
                    .join("migration-support/generated-scripts")
                    .join(name)
                    .join(relative);
                migration.add_move(source.clone(), target)?;
                continue;
            }
        }
        let target = migration.root.join(super::AUTHORING_ROOT).join(&authored);
        let key = target.to_string_lossy().to_ascii_lowercase();
        if let Some(previous) = targets.insert(key, source.clone()) {
            return Err(problem(format!(
                "migration collision: `{}` and `{}` both become `{}`",
                previous.display(),
                source.display(),
                target.display()
            )));
        }
        if source != target {
            if let Some(first) = source
                .strip_prefix(&migration.root)
                .ok()
                .and_then(|relative| relative.components().next())
            {
                let directory = migration.root.join(first.as_os_str());
                if directory.is_dir()
                    && directory != migration.root.join(super::AUTHORING_ROOT)
                    && !migration.cleanup.contains(&directory)
                {
                    migration.cleanup.push(directory);
                }
            }
            migration.add_move(source.clone(), target.clone())?;
        }
        let old_reference = if modern {
            relative
                .strip_prefix(&format!("{}/", migration.reference_root))
                .unwrap_or(&relative)
        } else {
            &relative
        };
        if old_reference != authored {
            mappings.push((old_reference.to_owned(), authored.clone()));
        }
        if let Some((_, _, _, value)) =
            module.filter(|_| source.file_name().is_some_and(|n| n == "ara.module.toml"))
        {
            migration.rewrites.push(Rewrite {
                source: source.clone(),
                target,
                before: fs::read(&source).map_err(io_error)?,
                after: ara_json::to_canonical_string(value).into_bytes(),
                existed: true,
            });
        }
    }
    let project_root = migration.root.clone();
    empty_directories(migration, &project_root, modern)?;
    // Prefixes also cover references in documentation, disabled records and missing media.
    for (from, to) in [
        ("content/assets", ""),
        ("game", ""),
        ("content", ""),
        ("assets", ""),
        (super::AUTHORING_ROOT, ""),
    ] {
        if !modern {
            mappings.push((from.to_owned(), to.to_owned()));
        }
    }
    mappings.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    // Empty legacy folders are removed only after all files have moved successfully.
    for directory in super::migrate::CONTENT_DIRS
        .iter()
        .copied()
        .chain(["game", "content", "scripts"])
    {
        let path = migration.root.join(directory);
        if path.is_dir() && directory != super::AUTHORING_ROOT {
            migration.cleanup.push(path);
        }
    }
    Ok(mappings)
}

fn relative(root: &Path, path: &Path) -> Result<String, Diagnostic> {
    relative_path(root, path)
}
fn relative_path(root: &Path, path: &Path) -> Result<String, Diagnostic> {
    Ok(path
        .strip_prefix(root)
        .map_err(|e| problem(e.to_string()))?
        .to_string_lossy()
        .replace('\\', "/"))
}
fn authored_path(path: &str, modern: bool) -> Option<String> {
    for prefix in [
        format!("{}/", super::AUTHORING_ROOT),
        "content/assets/".into(),
        "game/".into(),
        "content/".into(),
        "assets/".into(),
    ] {
        if let Some(path) = path.strip_prefix(&prefix) {
            return Some(path.to_owned());
        }
    }
    let first = path.split('/').next()?;
    if [
        "settings",
        "migration-support",
        "presets",
        "evidence",
        "tests",
        "packages",
        "builds",
        "target",
        "node_modules",
        "spikes",
    ]
    .contains(&first)
    {
        return None;
    }
    if super::migrate::CONTENT_DIRS.contains(&first)
        || (!modern
            && (path.contains('/')
                || [
                    "scene",
                    "prefab",
                    "cs",
                    "types.json",
                    "module.json",
                    "graph",
                    "ui",
                    "css",
                    "look",
                    "input",
                    "vfx",
                    "terrain",
                    "foliage",
                    "animation",
                    "music",
                    "mixer",
                    "png",
                    "jpg",
                    "glb",
                    "gltf",
                    "fbx",
                    "wav",
                    "ogg",
                ]
                .iter()
                .any(|extension| path.ends_with(&format!(".{extension}")))))
    {
        Some(path.to_owned())
    } else {
        None
    }
}

fn empty_directories(
    migration: &mut Migration,
    directory: &Path,
    modern: bool,
) -> Result<(), Diagnostic> {
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_dir() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.')
            || (directory == migration.root.as_path()
                && [
                    "builds",
                    "packages",
                    "target",
                    "settings",
                    "evidence",
                    "tests",
                    "migration-support",
                ]
                .iter()
                .any(|n| name == *n))
        {
            continue;
        }
        if path.join(crate::project::MARKER).is_file() {
            continue;
        }
        if fs::read_dir(&path).map_err(io_error)?.next().is_none() {
            let relative = relative_path(&migration.root, &path)?;
            if let Some(authored) = authored_path(&format!("{relative}/"), modern) {
                let target = migration.root.join(super::AUTHORING_ROOT).join(authored);
                if path != target {
                    if target.exists() && !target.is_dir() {
                        return Err(problem(format!(
                            "migration directory collides with file {}",
                            target.display()
                        )));
                    }
                    // Moving files has already created their destination ancestors.
                    // An empty legacy root can therefore merge into that directory.
                    let target_key = super::migrate::path_key(&target);
                    if migration.moves.iter().any(|(source, planned)| {
                        source.is_file() && super::migrate::path_key(planned) == target_key
                    }) {
                        return Err(problem(format!(
                            "migration directory collides with planned file {}",
                            target.display()
                        )));
                    }
                    let directory_planned = migration.moves.iter().any(|(_, planned)| {
                        let planned = super::migrate::path_key(planned);
                        planned == target_key || planned.starts_with(&format!("{target_key}/"))
                    });
                    if !target.exists() && !directory_planned {
                        migration.add_move(path, target)?;
                    }
                }
            }
        } else {
            empty_directories(migration, &path, modern)?;
        }
    }
    Ok(())
}
