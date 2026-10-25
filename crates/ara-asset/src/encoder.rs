//! Explicit encoding services keep device choice out of pure importer inputs.
use crate::{CacheBackend, Cancellation, CookPreset, Digest, ImportInput, Result};
use ara_asset_format::TextureFormat;

/// Worker-installed block encoder. Its immutable identity includes the algorithm
/// and cooked shader revision, so different implementations cannot alias caches.
pub trait BlockEncoder: Send + Sync {
    /// Stable, portable content identity; never a device name or filesystem path.
    fn backend(&self) -> &str;
    /// Cumulative offline telemetry; excluded from cache identity and artifacts.
    fn metrics(&self) -> std::collections::BTreeMap<String, f64> {
        Default::default()
    }
    /// Encodes block-major texels: 16 RGBA8 words, or 32 RGB-half words for BC6H,
    /// per block. Output blocks retain input order, independently of scheduling.
    fn encode(
        &self,
        format: TextureFormat,
        preset: CookPreset,
        blocks: &[u32],
        cancellation: &Cancellation,
    ) -> Result<Vec<u8>>;
}

/// Optional worker services. Missing services preserve the original CPU path.
#[derive(Clone, Copy, Default)]
pub struct ImportServices<'a> {
    /// Disposable content-addressed intermediate work cache.
    pub memo: Option<&'a dyn CacheBackend>,
    /// An explicitly selected deterministic block encoder; None uses CPU ISPC.
    pub encoder: Option<&'a dyn BlockEncoder>,
}

impl ImportInput {
    /// The action key for an explicitly selected encoder. CPU callers retain the
    /// existing key; a GPU failure must retry with the CPU key before publication.
    pub fn key_with_backend(&self, backend: Option<&str>) -> Result<Digest> {
        let key = self.key()?;
        let Some(backend) = backend else {
            return Ok(key);
        };
        let mut hash = blake3::Hasher::new_derive_key("Ara encoded import action v1");
        hash.update(&key.0);
        hash.update(&(backend.len() as u64).to_le_bytes());
        hash.update(backend.as_bytes());
        Ok(Digest(*hash.finalize().as_bytes()))
    }
}
