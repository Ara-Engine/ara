use serde::{Deserialize, Serialize};

/// Encoding quality is an explicit cook input, independent of the execution budget.
/// New imports default to Balanced; projects and individual textures can select HQ.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ara_schema::Schema,
)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase")]
pub enum CookPreset {
    /// Balanced compression search, the default for interactive project imports.
    #[default]
    Balanced,
    /// Deeper compression search for higher-quality assets and background recooks.
    HighQuality,
    /// Reduced compression search for the first interactive generation.
    Fast,
}

/// Default worker concurrency. Editors can lower it without changing cache identity.
pub const DEFAULT_THREAD_BUDGET: u16 = 16;

pub(crate) fn default_thread_budget() -> u16 {
    DEFAULT_THREAD_BUDGET
}
