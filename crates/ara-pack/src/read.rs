use std::io::Read;
use std::path::Path;

use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305,
    aead::{Aead, Payload},
};

use crate::{
    ALIGN, EncryptionKey, Error, HEADER_LEN, MAGIC, MAX_CHUNK, Manifest, VERSION, canonical,
    invalid,
    model::{nonce, valid_hash},
    validate_path,
};

/// Validated borrowed pack. Header, every range and stored chunk hash are checked
/// on open; decompressed identities are checked when read or by `validate_all`.
pub struct Pack<'a> {
    bytes: &'a [u8],
    data: usize,
    manifest: Manifest,
}

impl<'a> Pack<'a> {
    /// Opens untrusted bytes with bounded manifest/decompression allocations.
    pub fn open(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
            return Err(invalid("bad pack header"));
        }
        let u32_at = |at| -> Result<u32, Error> {
            Ok(u32::from_le_bytes(
                bytes[at..at + 4]
                    .try_into()
                    .map_err(|_| invalid("short header"))?,
            ))
        };
        let u64_at = |at| -> Result<u64, Error> {
            Ok(u64::from_le_bytes(
                bytes[at..at + 8]
                    .try_into()
                    .map_err(|_| invalid("short header"))?,
            ))
        };
        if u32_at(8)? != VERSION || u32_at(12)? != 0 {
            return Err(invalid("unsupported version or flags"));
        }
        let len = usize::try_from(u64_at(16)?).map_err(|_| invalid("manifest length overflow"))?;
        if len > 16 * 1024 * 1024 {
            return Err(invalid("manifest exceeds 16 MiB"));
        }
        let end = HEADER_LEN
            .checked_add(len)
            .ok_or_else(|| invalid("manifest overflow"))?;
        let json = bytes
            .get(HEADER_LEN..end)
            .ok_or_else(|| invalid("manifest outside file"))?;
        if blake3::hash(json).as_bytes() != &bytes[32..64] {
            return Err(invalid("manifest hash mismatch"));
        }
        let value = ara_json::parse(std::str::from_utf8(json).map_err(|e| invalid(e.to_string()))?)
            .map_err(|e| invalid(e.to_string()))?;
        let manifest: Manifest = serde_json::from_value(value)?;
        if canonical(&manifest)? != json {
            return Err(invalid("noncanonical manifest"));
        }
        if manifest.format != "ara.pack/2"
            || manifest.chunk_size == 0
            || manifest.chunk_size as usize > MAX_CHUNK
            || !matches!(
                manifest.encoding.as_str(),
                "zstd-9" | "zstd-9+xchacha20poly1305"
            )
        {
            return Err(invalid("unsupported pack settings"));
        }
        let data = usize::try_from(u64_at(24)?).map_err(|_| invalid("data overflow"))?;
        if data < end
            || data > bytes.len()
            || data % ALIGN != 0
            || bytes[end..data].iter().any(|b| *b != 0)
        {
            return Err(invalid("invalid data offset or padding"));
        }
        let pack = Self {
            bytes,
            data,
            manifest,
        };
        let mut previous_end = 0u64;
        for (id, c) in &pack.manifest.chunks {
            if !valid_hash(id)
                || !valid_hash(&c.stored_hash)
                || c.raw == 0
                || c.raw > u64::from(pack.manifest.chunk_size)
                || c.stored == 0
                || c.stored > (MAX_CHUNK + 65536) as u64
                || c.offset % ALIGN as u64 != 0
                || c.offset < previous_end
            {
                return Err(invalid("invalid chunk metadata/order"));
            }
            let stored = pack.stored_chunk(id)?;
            if blake3::hash(stored).to_hex().as_str() != c.stored_hash {
                return Err(invalid(format!("stored chunk hash mismatch: {id}")));
            }
            previous_end = c
                .offset
                .checked_add(c.stored)
                .ok_or_else(|| invalid("chunk overflow"))?;
        }
        for (path, entry) in &pack.manifest.entries {
            validate_path(path)?;
            if !valid_hash(&entry.hash) {
                return Err(invalid("invalid file hash"));
            }
            let mut size = 0u64;
            for (i, id) in entry.chunks.iter().enumerate() {
                let c = pack
                    .manifest
                    .chunks
                    .get(id)
                    .ok_or_else(|| invalid("missing entry chunk"))?;
                if i + 1 != entry.chunks.len() && c.raw != u64::from(pack.manifest.chunk_size) {
                    return Err(invalid("short interior chunk"));
                }
                size = size
                    .checked_add(c.raw)
                    .ok_or_else(|| invalid("file size overflow"))?;
            }
            if size != entry.size {
                return Err(invalid("file length mismatch"));
            }
        }
        Ok(pack)
    }

    /// The immutable validated manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Pack identity, excluding alignment and header bytes.
    pub fn id(&self) -> Result<String, Error> {
        self.manifest.id()
    }

    /// Returns compressed ciphertext/plaintext for content-addressed store delivery.
    pub fn stored_chunk(&self, id: &str) -> Result<&'a [u8], Error> {
        let c = self
            .manifest
            .chunks
            .get(id)
            .ok_or_else(|| invalid("unknown chunk"))?;
        let start = (self.data as u64)
            .checked_add(c.offset)
            .and_then(|s| usize::try_from(s).ok())
            .ok_or_else(|| invalid("chunk offset overflow"))?;
        let len = usize::try_from(c.stored).map_err(|_| invalid("chunk length overflow"))?;
        self.bytes
            .get(
                start
                    ..start
                        .checked_add(len)
                        .ok_or_else(|| invalid("chunk range overflow"))?,
            )
            .ok_or_else(|| invalid("chunk outside file"))
    }

    /// Decompresses and verifies one chunk with a hard output limit.
    pub fn read_chunk(&self, id: &str, key: Option<&EncryptionKey>) -> Result<Vec<u8>, Error> {
        let c = self
            .manifest
            .chunks
            .get(id)
            .ok_or_else(|| invalid("unknown chunk"))?;
        let stored = self.stored_chunk(id)?;
        let decrypted;
        let compressed = if self.manifest.encoding.ends_with("xchacha20poly1305") {
            let key = key.ok_or(Error::Key)?;
            decrypted = XChaCha20Poly1305::new((&key.0).into())
                .decrypt(
                    (&nonce(key, id)).into(),
                    Payload {
                        msg: stored,
                        aad: id.as_bytes(),
                    },
                )
                .map_err(|_| Error::Key)?;
            &decrypted[..]
        } else {
            stored
        };
        let decoder = ruzstd::decoding::StreamingDecoder::new(compressed)
            .map_err(|e| invalid(e.to_string()))?;
        let mut raw = Vec::new();
        decoder.take(c.raw + 1).read_to_end(&mut raw)?;
        if raw.len() as u64 != c.raw || blake3::hash(&raw).to_hex().as_str() != id {
            return Err(invalid("raw chunk hash/length mismatch"));
        }
        Ok(raw)
    }

    /// Reconstructs a file with caller-controlled size limits and verifies its hash.
    pub fn read(
        &self,
        path: &str,
        limit: u64,
        key: Option<&EncryptionKey>,
    ) -> Result<Vec<u8>, Error> {
        let entry = self
            .manifest
            .entries
            .get(path)
            .ok_or_else(|| invalid(format!("missing @{path}")))?;
        if entry.size > limit {
            return Err(invalid("file exceeds caller's size limit"));
        }
        let mut bytes = Vec::new();
        for id in &entry.chunks {
            bytes.extend_from_slice(&self.read_chunk(id, key)?);
        }
        if blake3::hash(&bytes).to_hex().as_str() != entry.hash {
            return Err(invalid("file hash mismatch"));
        }
        Ok(bytes)
    }

    /// Checks all plaintext chunk identities without allocating entire files.
    pub fn validate_all(&self, key: Option<&EncryptionKey>) -> Result<(), Error> {
        for id in self.manifest.chunks.keys() {
            self.read_chunk(id, key)?;
        }
        for entry in self.manifest.entries.values() {
            let mut hash = blake3::Hasher::new();
            for id in &entry.chunks {
                hash.update(&self.read_chunk(id, key)?);
            }
            if hash.finalize().to_hex().as_str() != entry.hash {
                return Err(invalid("file hash mismatch"));
            }
        }
        Ok(())
    }
}

/// Owned read-only OS mapping. Parse views borrow it and cannot outlive the mapping.
pub struct MappedPack(memmap2::Mmap);

impl MappedPack {
    /// Maps and validates a pack without copying the stored payload.
    ///
    /// # Safety
    /// The caller must ensure no process modifies or truncates this file until
    /// the mapping is dropped. Publish immutable content-addressed files first.
    pub unsafe fn open(path: &Path) -> Result<Self, Error> {
        let file = std::fs::File::open(path)?;
        // SAFETY: caller guarantees the backing file stays immutable for the mapping lifetime.
        let mapped = unsafe { memmap2::MmapOptions::new().map(&file)? };
        Pack::open(&mapped)?;
        Ok(Self(mapped))
    }

    /// Revalidates the mapped header and returns a view with validated chunk ranges.
    pub fn pack(&self) -> Result<Pack<'_>, Error> {
        Pack::open(&self.0)
    }
}
