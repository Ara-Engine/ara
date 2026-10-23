//! Out-of-process importer entry point; uses ara-asset framing and low priority.
fn main() -> ara_asset::Result<()> {
    ara_asset::serve_worker(&ara_import2d::registry()?)
}
