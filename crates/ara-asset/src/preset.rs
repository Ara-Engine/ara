use serde::{Deserialize, Serialize};

/// Encoding quality is an explicit cook input, independent of the execution budget.
/// Direct/ shipping cooks default to high quality; editor previews use `Fast`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ara_schema::Schema,
)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase")]
pub enum CookPreset {
    /// ISPC BC7 slow and BC6H very slow, for background recooks and shipping.
    #[default]
    HighQuality,
    /// ISPC BC7 ultrafast and BC6H fast, for the first interactive generation.
    Fast,
    /// ISPC BC7/BC6H basic, for projects choosing a middle quality/speed tradeoff.
    Balanced,
}

/// Default worker concurrency. Editors can lower it without changing cache identity.
pub const DEFAULT_THREAD_BUDGET: u16 = 16;

pub(crate) fn default_thread_budget() -> u16 {
    DEFAULT_THREAD_BUDGET
}
