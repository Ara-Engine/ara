//! Player isolation modes; both use the same per-world wire/session contract.
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Where the editor's independently owned Play worlds execute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessMode {
    /// One player process contains separate world/input/RNG/session owners.
    Shared,
    /// Each player has its own process and process-crash boundary.
    #[default]
    Separate,
}
