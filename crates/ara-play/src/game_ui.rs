//! Portable native game UI sources and schema-checked C# field subscriptions.
use serde::{Deserialize, Serialize};

/// Authored `ui/game.ui`, `ui/game.css` and `ui/bindings.json` carried inside Play/export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameUi {
    /// Project-relative resource paths keyed by `image:<name>`, `font:<index>` or `click`.
    /// Retained across Play/export so resource diagnostics identify the authored file.
    #[serde(default)]
    pub resource_paths: std::collections::BTreeMap<String, String>,
    /// Nonfatal resource-read failures; presentation reports them and keeps the HUD usable.
    #[serde(default)]
    pub resource_diagnostics: Vec<String>,
    /// Explicit licensed font bytes, packaged so exported games never use OS font discovery.
    #[serde(default)]
    pub fonts: Vec<Vec<u8>>,
    /// Named PNG image resources used by the markup (icons and input prompts).
    #[serde(default)]
    pub images: std::collections::BTreeMap<String, Vec<u8>>,
    /// Reliable presentation counters from typed C# components.
    #[serde(default)]
    pub sounds: Vec<UiSound>,
    /// Replicated gameplay fields that retarget presentation music intensity.
    #[serde(default)]
    pub music: Vec<UiMusic>,
    /// Licensed compressed UI click, packaged for the menu before the world loads.
    #[serde(default)]
    pub click: Vec<u8>,
    /// Native markup source.
    pub markup: String,
    /// Native stylesheet source.
    pub stylesheet: String,
    /// Typed C# component-field subscriptions.
    pub bindings: Vec<UiBinding>,
}

/// A replicated monotonic C# counter triggers presentation once per changed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiSound {
    /// C# component carrying the event counter.
    pub component: String,
    /// Counter field (a simulation tick or increasing sequence).
    pub field: String,
    /// A cooked audio reference, also pinned by an authored AudioSource in the scene.
    pub clip: ara_schema::AssetRef,
    /// Source amplitude before the game's SFX bus preference.
    #[serde(default = "one")]
    pub gain: f32,
    /// Cooked native particle effect to flash at the source, when present.
    #[serde(default)]
    pub effect: Option<String>,
    /// Hides the source model once the counter is nonzero (consumable pickups).
    #[serde(default)]
    pub hide_source: bool,
    /// Optional boolean animation input held briefly after this counter changes.
    #[serde(default)]
    pub animation: Option<String>,
}
fn one() -> f32 {
    1.
}

/// A numeric replicated field drives the score on a named MusicSource entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiMusic {
    /// Entity carrying the reflected gameplay component.
    pub entity: String,
    /// Reflected C# component name.
    pub component: String,
    /// Numeric field, clamped to the score's [0,1] intensity range.
    pub field: String,
    /// Entity carrying the native MusicSource.
    pub target: String,
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
    /// Display fixed 60 Hz ticks as minutes and seconds.
    #[serde(default)]
    pub clock: bool,
    /// Localized labels keyed by the serialized value, for enum-like prompts.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
    /// Anchor this control above its entity in the current camera's view.
    #[serde(default)]
    pub world: bool,
    /// Boolean component field hiding inactive world anchors.
    #[serde(default)]
    pub active: Option<String>,
}

fn value_template() -> String {
    "{value}".into()
}
