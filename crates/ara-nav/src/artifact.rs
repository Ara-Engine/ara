//! Immutable, serializable bake artifacts and content identity.
use crate::{BakeSettings, OffMeshLink, TileCoord};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

/// BLAKE3 identity of canonical tile inputs, settings and native format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CacheKey(pub [u8; 32]);
impl std::fmt::Display for CacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// An immutable owned tile. Bytes are private and validated again by the native loader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileData {
    /// Canonical input identity.
    pub key: CacheKey,
    pub(crate) bytes: Arc<[u8]>,
}
impl TileData {
    /// Native tile size in bytes; zero represents an empty tile (a tombstone).
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}

/// Work counts, deliberately excluding wall-clock time from deterministic artifacts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BakeStats {
    /// Tiles whose input changed and required Recast.
    pub rebuilt: usize,
    /// Tiles reused from the previous artifact.
    pub reused: usize,
    /// Tiles loaded from disk by content identity.
    pub cache_hits: usize,
    /// Previously loaded tiles omitted by this request.
    pub removed: usize,
}

/// Complete immutable result, transferable from a worker without native pointers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BakeArtifact {
    /// Settings shared by every tile.
    pub settings: BakeSettings,
    /// Loaded tiles in deterministic insertion order.
    pub tiles: BTreeMap<TileCoord, TileData>,
    /// Links retained for reachability explanations.
    pub links: Vec<OffMeshLink>,
    /// Worker effort for this revision.
    pub stats: BakeStats,
}
impl BakeArtifact {
    /// Deterministic hash of the published content, independent of cache/rebuild history.
    pub fn content_hash(&self) -> CacheKey {
        let mut h = blake3::Hasher::new();
        let metadata = postcard::to_allocvec(&(&self.settings, &self.links))
            .expect("navigation metadata serializes into an unbounded Vec");
        h.update(&metadata);
        for (coord, tile) in &self.tiles {
            h.update(&coord.x.to_le_bytes());
            h.update(&coord.z.to_le_bytes());
            h.update(&tile.key.0);
            h.update(&tile.bytes);
        }
        CacheKey(*h.finalize().as_bytes())
    }
}
