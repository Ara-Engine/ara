//! Offline importer host used by examples and integration verification.
fn main() -> ara_asset::Result<()> {
    ara_asset::serve_worker(&ara_import2d::registry()?)
}
