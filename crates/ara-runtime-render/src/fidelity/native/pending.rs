//! Editor publication is incremental; absent generations are distinct from bad cooks.
use ara_runtime::assets::AssetSource;
use std::sync::atomic::{AtomicBool, Ordering};

/// Resolves ready content, or returns `None` when resolution reads an unpublished
/// asset. Malformed published generations and unrelated errors remain errors.
/// Use only at editor content boundaries; Play/export keep strict asset pinning.
pub fn when_published<T>(
    source: &dyn AssetSource,
    resolve: impl FnOnce(&dyn AssetSource) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    let reader = Reader {
        source,
        pending: AtomicBool::new(false),
    };
    match resolve(&reader) {
        Ok(value) => Ok(Some(value)),
        Err(_) if reader.pending.load(Ordering::Relaxed) => Ok(None),
        Err(error) => Err(error),
    }
}

struct Reader<'a> {
    source: &'a dyn AssetSource,
    pending: AtomicBool,
}
impl AssetSource for Reader<'_> {
    fn digest(&self, asset: &str) -> Option<ara_asset::Digest> {
        self.source.digest(asset)
    }
    fn load(
        &self,
        asset: &str,
    ) -> ara_runtime::Result<Option<(ara_asset::Digest, ara_asset::CachedImport)>> {
        let value = self.source.load(asset)?;
        if value.is_none() && self.source.digest(asset).is_none() {
            self.pending.store(true, Ordering::Relaxed);
        }
        Ok(value)
    }
}
