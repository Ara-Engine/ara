use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Error, canonical, invalid};

/// Pack header magic, followed by version 2, flags, manifest length, data offset and hash.
pub const MAGIC: &[u8; 8] = b"ARAPACK\0";
/// Version of the on-disk format.
pub const VERSION: u32 = 2;
/// Header length in bytes.
pub const HEADER_LEN: usize = 64;
/// Chunk alignment for mapping and store patching.
pub const ALIGN: usize = 4096;
/// Maximum uncompressed chunk size accepted by the reader (16 MiB).
pub const MAX_CHUNK: usize = 16 * 1024 * 1024;

/// Inputs that distinguish platform-specific cooked data and importer caches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CookKey {
    /// Destination OS, for example `windows` or `android`.
    pub platform: String,
    /// Destination CPU or `universal` for shared multi-ABI data.
    pub arch: String,
    /// Runtime configuration: `dev`, `profile` or `release`.
    pub config: String,
    /// Version of the pure cooker.
    pub cooker: String,
    /// Content-affecting settings; source paths must never be included.
    pub settings: BTreeMap<String, String>,
}

impl CookKey {
    /// Hashes all cook inputs in Ara's canonical JSON form.
    pub fn digest(&self) -> Result<String, Error> {
        Ok(blake3::hash(&canonical(self)?).to_hex().to_string())
    }
}

/// Logical file reconstructed by concatenating its ordered chunk IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Hash of the complete cooked file.
    pub hash: String,
    /// Total uncompressed length.
    pub size: u64,
    /// Chunk IDs in file order (can repeat).
    pub chunks: Vec<String>,
}

/// Placement and validation metadata for one deduplicated chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    /// Relative byte offset from the aligned data region.
    pub offset: u64,
    /// Stored length, including encryption tag when present.
    pub stored: u64,
    /// Uncompressed length, bounded by MAX_CHUNK.
    pub raw: u64,
    /// Hash of the stored bytes, permitting validation without a key.
    pub stored_hash: String,
}

/// Content identity and layout; timestamps and source roots are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Always `ara.pack/2`.
    pub format: String,
    /// Platform-specific cook inputs.
    pub cook: CookKey,
    /// Fixed raw chunk size used by this pack.
    pub chunk_size: u32,
    /// `zstd-9` or `zstd-9+xchacha20poly1305`.
    pub encoding: String,
    /// Logical paths to cooked file descriptions.
    pub entries: BTreeMap<String, Entry>,
    /// Raw content hash to compressed/encrypted chunk placement.
    pub chunks: BTreeMap<String, Chunk>,
}

impl Manifest {
    /// Pack identity: BLAKE3 of the canonical manifest.
    pub fn id(&self) -> Result<String, Error> {
        Ok(blake3::hash(&canonical(self)?).to_hex().to_string())
    }
}

/// Encryption key supplied by the caller; debug formatting never reveals bytes.
#[derive(Clone)]
pub struct EncryptionKey(pub [u8; 32]);

impl EncryptionKey {
    /// Parses a caller-supplied 256-bit key without accepting Unicode or leaking it in errors.
    pub fn from_hex(text: &str) -> Result<Self, Error> {
        if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Key);
        }
        let mut key = [0; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| Error::Key)?;
        }
        Ok(Self(key))
    }
}

impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EncryptionKey([redacted])")
    }
}

/// Store delta: deliver the target manifest plus only chunks absent in the base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkDiff {
    /// Required base manifest identity.
    pub base: String,
    /// Complete target manifest identity.
    pub target: String,
    /// Target manifest used to assemble the installed pack.
    pub manifest: Manifest,
    /// Stored chunk IDs to download; includes changed encryption/compression.
    pub added: BTreeSet<String>,
    /// Chunk IDs no longer referenced by the target.
    pub removed: BTreeSet<String>,
    /// Sum of the download chunk sizes, excluding the manifest.
    pub download_bytes: u64,
}

/// Computes a delivery delta without reading or decompressing any chunk bytes.
pub fn diff(base: &Manifest, target: &Manifest) -> Result<ChunkDiff, Error> {
    if base.cook != target.cook || base.encoding != target.encoding {
        return Err(invalid("cannot diff different cook keys or encodings"));
    }
    let added: BTreeSet<_> = target
        .chunks
        .iter()
        .filter(|(id, c)| {
            base.chunks
                .get(*id)
                .is_none_or(|old| old.stored_hash != c.stored_hash)
        })
        .map(|(id, _)| id.clone())
        .collect();
    let removed = base
        .chunks
        .keys()
        .filter(|id| !target.chunks.contains_key(*id))
        .cloned()
        .collect();
    let download_bytes = added.iter().map(|id| target.chunks[id].stored).sum();
    Ok(ChunkDiff {
        base: base.id()?,
        target: target.id()?,
        manifest: target.clone(),
        added,
        removed,
        download_bytes,
    })
}

pub(crate) fn nonce(key: &EncryptionKey, id: &str) -> [u8; 24] {
    let hash = blake3::keyed_hash(&key.0, format!("ara.pack/2:zstd-9:{id}").as_bytes());
    let mut out = [0; 24];
    out.copy_from_slice(&hash.as_bytes()[..24]);
    out
}

pub(crate) fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
