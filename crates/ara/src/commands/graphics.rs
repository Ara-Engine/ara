//! Local presentation preferences through the existing authenticated Engine Protocol.
use super::protocol::{self, Call};
use ara_diag::Diagnostic;
use serde_json::{Value, json};

/// `ara graphics [operation] [--settings @file.json] [--world play:1] [--player N]`.
#[derive(clap::Args)]
pub struct GraphicsArgs {
    /// get/set/apply/revert/resetToDefaults/save/autoDetect; get is read-only.
    #[arg(default_value = "get", value_parser = ["get", "set", "apply", "revert", "resetToDefaults", "save", "autoDetect"])]
    operation: String,
    /// Complete candidate JSON, @file.json, or - for stdin (required for set).
    #[arg(long)]
    settings: Option<String>,
    /// Active play world; absent selects the latest world.
    #[arg(long)]
    world: Option<String>,
    /// One player; absent selects the authority. Preferences are never broadcast.
    #[arg(long)]
    player: Option<u32>,
}

impl GraphicsArgs {
    /// Construct the same typed graphics request used by C# game menus.
    pub fn call(self) -> Result<Call, Vec<Diagnostic>> {
        let settings = match (self.operation.as_str(), self.settings) {
            ("set", Some(text)) => Some(protocol::args(Some(&text), &[])?),
            ("set", None) => {
                return Err(vec![Diagnostic::error(
                    ara_diag::codes::INVALID_PARAMS,
                    "graphics set requires --settings JSON or @file.json",
                )]);
            }
            (_, Some(_)) => {
                return Err(vec![Diagnostic::error(
                    ara_diag::codes::INVALID_PARAMS,
                    "--settings is only valid with graphics set",
                )]);
            }
            (_, None) => None,
        };
        let mut request = json!({"operation":self.operation});
        if let Some(settings) = settings {
            request["settings"] = settings;
        }
        let mut params = json!({"request":request});
        if let Some(world) = self.world {
            params["world"] = json!(world);
        }
        if let Some(player) = self.player {
            params["player"] = json!(player);
        }
        Ok(Call {
            method: "play.graphics",
            params,
            summary,
        })
    }
}

fn summary(value: &Value) -> String {
    let state = &value["state"];
    let applied = &state["applied"];
    format!(
        "Graphics: {} (revision {}); {}x{}, {}, vsync {}; FPS cap {}. Ray tracing {}; restart {}. {} display modes. Use --json for all preferences and capabilities.",
        applied["preset"].as_str().unwrap_or("unknown"),
        state["revision"],
        applied["width"],
        applied["height"],
        applied["windowMode"].as_str().unwrap_or("unknown"),
        applied["vsync"],
        applied["frameRateCap"],
        state["activeRayTracing"],
        state["restartRequired"],
        state["capabilities"]["modes"]
            .as_array()
            .map_or(0, Vec::len)
    )
}
