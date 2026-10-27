//! Pure, explicit operation identities: preview and commit need no mutable ID seed.
use super::invalid;
use ara_diag::Diagnostic;
use ara_doc::file_state::FileState;
use ara_doc::{AssetId, RecordId};
use std::collections::BTreeSet;

fn fresh(
    prefix: &str,
    domain: &str,
    token: &str,
    path: &str,
    foreign: &str,
    taken: &mut BTreeSet<String>,
) -> Result<String, Diagnostic> {
    for ordinal in 0..=taken.len().saturating_add(1) {
        let value = serde_json::json!([
            "ara.asset.identity/1",
            domain,
            prefix,
            token,
            path,
            foreign,
            ordinal
        ]);
        let hash = FileState::bytes(ara_json::to_canonical_string(&value).as_bytes()).hash;
        let id = format!("{prefix}_{}", &hash[..20]);
        if taken.insert(id.clone()) {
            return Ok(id);
        }
    }
    Err(invalid(
        "cannot allocate a collision-free prepared asset identity",
    ))
}
pub(super) fn asset(
    prefix: &str,
    token: &str,
    path: &str,
    foreign: &str,
    taken: &mut BTreeSet<String>,
) -> Result<AssetId, Diagnostic> {
    AssetId::parse(&fresh(prefix, "asset", token, path, foreign, taken)?).map_err(invalid)
}
pub(super) fn record(
    prefix: &str,
    token: &str,
    path: &str,
    foreign: &str,
    taken: &mut BTreeSet<String>,
) -> Result<RecordId, Diagnostic> {
    RecordId::parse(&fresh(prefix, "record", token, path, foreign, taken)?).map_err(invalid)
}
