use super::*;
use ara_runtime::assets::MemoryAssets;

#[test]
fn pending_assets_do_not_hide_bad_published_generations() {
    let mut source = MemoryAssets::default();
    assert!(
        when_published(&source, |source| {
            let mut models = ara_runtime::assets::Models::default();
            Ok(models.load(source, "as_pending")?)
        })
        .unwrap()
        .is_none()
    );
    source.0.insert(
        "as_pending".into(),
        ara_asset::CachedImport {
            key: AssetDigest::of(b"invalid cook"),
            output: ara_asset::ImportOutput {
                artifacts: Vec::new(),
            },
        },
    );
    assert!(
        when_published(&source, |source| {
            let mut models = ara_runtime::assets::Models::default();
            Ok(models.load(source, "as_pending")?)
        })
        .is_err()
    );
}
