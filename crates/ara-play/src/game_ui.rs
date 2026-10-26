//! Portable native game UI sources and schema-checked C# field subscriptions.
use serde::{Deserialize, Serialize};

/// Authored `ui/game.ui`, `ui/game.css` and `ui/bindings.json` carried inside Play/export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameUi {
    /// Native markup source.
    pub markup: String,
    /// Native stylesheet source.
    pub stylesheet: String,
    /// Typed C# component-field subscriptions.
    pub bindings: Vec<UiBinding>,
}

/// A field subscription validated against the live schema before the first game frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiBinding {
    /// Authored control ID.
    pub node: String,
    /// Reflected C# component name.
    pub component: String,
    /// Reflected C# field name.
    pub field: String,
    /// `player` selects the local input owner's avatar; otherwise an entity identity.
    pub entity: String,
    /// Text surrounding the value; `{value}` is replaced at a notified tick boundary.
    #[serde(default = "value_template")]
    pub template: String,
    /// A boolean field controls visibility instead of text.
    #[serde(default)]
    pub visibility: bool,
}

fn value_template() -> String { "{value}".into() }
