//! Packages native UI sources alongside their C# schema bindings.
use ara_play::GameUi;
use std::path::Path;
mod paths;

/// Reads portable game UI sources and explicit resources inside the project.
pub fn read(root: &Path) -> Result<Option<GameUi>, String> {
    let authored = ara_host::project_layout::authored_root(root);
    let preferences =
        ara_host::project_layout::preferences(root).map_err(|error| error.to_string())?;
    let configured = preferences.get("ui").is_some();
    let policy: paths::Policy = preferences
        .get("ui")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("project UI settings: {error}"))?
        .unwrap_or_default();
    let main = policy.main.as_deref().unwrap_or("ui/game.ui");
    if !configured && !authored.join(main).exists() {
        return Ok(None);
    }
    let paths = paths::Paths::new(root, &authored)?;
    #[derive(serde::Deserialize, Default)]
    #[serde(default, deny_unknown_fields)]
    struct Resources {
        fonts: Vec<String>,
        images: std::collections::BTreeMap<String, String>,
        sounds: Vec<ara_play::UiSound>,
        music: Vec<ara_play::UiMusic>,
        click: Option<String>,
    }
    let resources_path = policy.resources.as_deref().unwrap_or("ui/resources.json");
    let resources: Resources = match paths.resolve(resources_path) {
        Ok((_, path)) => serde_json::from_str(
            &std::fs::read_to_string(path)
                .map_err(|error| format!("UI resources {resources_path}: {error}"))?,
        )
        .map_err(|error| error.to_string())?,
        Err(paths::Error::Missing(_)) if policy.resources.is_none() => Resources::default(),
        Err(error) => return Err(error.message()),
    };
    let mut resource_paths = std::collections::BTreeMap::new();
    let mut resource_diagnostics = Vec::new();
    let mut load = |key: String, value: &str| -> Result<Option<Vec<u8>>, String> {
        let (relative, path) = match paths.resolve(value) {
            Ok(resolved) => resolved,
            Err(paths::Error::Unsafe(error)) => return Err(error),
            Err(paths::Error::Missing(error)) => {
                resource_paths.insert(key, value.to_owned());
                resource_diagnostics.push(error);
                return Ok(None);
            }
        };
        resource_paths.insert(key, relative);
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) => {
                resource_diagnostics.push(format!("UI resource {value}: {error}"));
                Ok(None)
            }
        }
    };
    let fonts = resources
        .fonts
        .iter()
        .enumerate()
        .map(|(index, path)| load(format!("font:{index}"), path).map(Option::unwrap_or_default))
        .collect::<Result<_, _>>()?;
    let mut images = std::collections::BTreeMap::new();
    for (name, path) in resources.images {
        if let Some(bytes) = load(format!("image:{name}"), &path)? {
            images.insert(name, bytes);
        }
    }
    let click = resources
        .click
        .as_deref()
        .map(|path| load("click".into(), path))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    Ok(Some(GameUi {
        resource_paths,
        resource_diagnostics,
        fonts,
        images,
        sounds: resources.sounds,
        music: resources.music,
        click,
        markup: paths.text(main)?,
        stylesheet: paths.text(policy.stylesheet.as_deref().unwrap_or("ui/game.css"))?,
        bindings: serde_json::from_str(
            &paths.text(policy.bindings.as_deref().unwrap_or("ui/bindings.json"))?,
        )
        .map_err(|e| e.to_string())?,
    }))
}
