//! Packages native UI sources alongside their C# schema bindings.
use ara_play::GameUi;
use std::path::Path;

pub(crate) fn read(root: &Path) -> Result<Option<GameUi>, String> {
    let dir = root.join("ui");
    if !dir.join("game.ui").is_file() { return Ok(None); }
    let read = |name| std::fs::read_to_string(dir.join(name)).map_err(|e| format!("ui/{name}: {e}"));
    Ok(Some(GameUi {
        markup: read("game.ui")?,
        stylesheet: read("game.css")?,
        bindings: serde_json::from_str(&read("bindings.json")?).map_err(|e| e.to_string())?,
    }))
}
