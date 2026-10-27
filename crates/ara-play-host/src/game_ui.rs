//! Packages native UI sources alongside their C# schema bindings.
use ara_play::GameUi;
use std::path::Path;

/// Reads portable game UI sources and explicit resources inside the project.
pub fn read(root: &Path) -> Result<Option<GameUi>, String> {
    let dir = root.join("ui");
    if !dir.join("game.ui").is_file() {
        return Ok(None);
    }
    let read =
        |name| std::fs::read_to_string(dir.join(name)).map_err(|e| format!("ui/{name}: {e}"));
    #[derive(serde::Deserialize, Default)]
    #[serde(default, deny_unknown_fields)]
    struct Resources {
        fonts: Vec<String>,
        images: std::collections::BTreeMap<String, String>,
        sounds: Vec<ara_play::UiSound>,
        music: Vec<ara_play::UiMusic>,
        click: Option<String>,
    }
    let resources: Resources = if dir.join("resources.json").is_file() {
        serde_json::from_str(&read("resources.json")?).map_err(|e| e.to_string())?
    } else {
        Resources::default()
    };
    let bytes = |path: &str| {
        if !Path::new(path)
            .components()
            .all(|p| matches!(p, std::path::Component::Normal(_)))
        {
            return Err(format!("UI resource must stay in the project: {path}"));
        }
        std::fs::read(root.join(path)).map_err(|e| format!("UI resource {path}: {e}"))
    };
    let mut resource_paths = std::collections::BTreeMap::new();
    let mut resource_diagnostics = Vec::new();
    let mut load = |key: String, path: &str| {
        resource_paths.insert(key, path.to_owned());
        match bytes(path) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                resource_diagnostics.push(error);
                None
            }
        }
    };
    let fonts = resources
        .fonts
        .iter()
        .enumerate()
        .map(|(index, path)| load(format!("font:{index}"), path).unwrap_or_default())
        .collect();
    let images = resources
        .images
        .into_iter()
        .filter_map(|(name, path)| load(format!("image:{name}"), &path).map(|bytes| (name, bytes)))
        .collect();
    let click = resources
        .click
        .as_deref()
        .and_then(|path| load("click".into(), path))
        .unwrap_or_default();
    Ok(Some(GameUi {
        resource_paths,
        resource_diagnostics,
        fonts,
        images,
        sounds: resources.sounds,
        music: resources.music,
        click,
        markup: read("game.ui")?,
        stylesheet: read("game.css")?,
        bindings: serde_json::from_str(&read("bindings.json")?).map_err(|e| e.to_string())?,
    }))
}
