//! Cooked assets at runtime: published import generations by stable asset ID, decoded
//! into immutable [`Model`]s. Decoding never happens inside a tick; generations swap
//! only at a tick boundary (D20), and a failed reload keeps the last good model.
use crate::{Result, RuntimeError, model::Model};
use ara_asset::{CacheBackend, CachedImport, Digest, FileCache};
use std::{collections::BTreeMap, sync::Arc};

/// Read access to published cooked generations. The edit-world owner pins which
/// generation is live; players never run importers.
pub trait AssetSource: Send + Sync {
    /// The live generation of an imported asset (`as_…` sidecar ID), if published.
    fn load(&self, asset: &str) -> Result<Option<(Digest, CachedImport)>>;
}

/// Generations in a content-addressed cache, pinned by action digest per asset ID.
pub struct CachedAssets {
    cache: FileCache,
    pins: BTreeMap<String, Digest>,
}

impl CachedAssets {
    /// Serves `pins` (asset ID → action digest) from `cache`.
    pub fn new(cache: FileCache, pins: BTreeMap<String, Digest>) -> Self {
        Self { cache, pins }
    }
    /// Pins a newly published generation (hot reload); takes effect on the next load.
    pub fn pin(&mut self, asset: &str, digest: Digest) {
        self.pins.insert(asset.to_owned(), digest);
    }
}

impl AssetSource for CachedAssets {
    fn load(&self, asset: &str) -> Result<Option<(Digest, CachedImport)>> {
        let Some(&digest) = self.pins.get(asset) else {
            return Ok(None);
        };
        Ok(self.cache.load(digest)?.map(|cached| (digest, cached)))
    }
}

/// A fixed set of in-memory generations, for tests and in-process simulation.
#[derive(Default)]
pub struct MemoryAssets(pub BTreeMap<String, CachedImport>);

impl AssetSource for MemoryAssets {
    fn load(&self, asset: &str) -> Result<Option<(Digest, CachedImport)>> {
        Ok(self.0.get(asset).map(|c| (c.key, c.clone())))
    }
}

/// Decoded models by asset ID. Shared immutable data; reloads replace whole entries.
#[derive(Default, Clone)]
pub struct Models {
    models: BTreeMap<String, Arc<Model>>,
}

impl Models {
    /// The decoded live model for an asset ID.
    pub fn get(&self, asset: &str) -> Option<&Arc<Model>> {
        self.models.get(asset)
    }
    /// Decodes `asset` from `source` unless that exact generation is already loaded.
    /// Returns whether a new generation was installed.
    pub fn load(&mut self, source: &dyn AssetSource, asset: &str) -> Result<bool> {
        let (digest, cached) = source
            .load(asset)?
            .ok_or_else(|| RuntimeError(format!("asset {asset} has no published generation")))?;
        if self.models.get(asset).is_some_and(|m| m.digest == digest) {
            return Ok(false);
        }
        let model = Model::decode(asset, digest, &cached)?;
        self.models.insert(asset.to_owned(), Arc::new(model));
        Ok(true)
    }
    /// Every loaded asset ID.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.models.keys().map(String::as_str)
    }
}

/// The asset ID of an `@path#id` reference.
pub fn asset_id(reference: &ara_schema::AssetRef) -> Result<&str> {
    ara_schema::grammar::parse_ref(&reference.0)
        .map_err(RuntimeError)?
        .id
        .ok_or_else(|| RuntimeError(format!("reference {} has no asset ID", reference.0)))
}
