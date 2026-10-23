use std::collections::BTreeMap;
use std::path::Path;

use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305,
    aead::{Aead, Payload},
};

use crate::{
    ALIGN, Chunk, CookKey, EncryptionKey, Entry, Error, HEADER_LEN, MAGIC, MAX_CHUNK, Manifest,
    VERSION, canonical, invalid, model::nonce, validate_path,
};

/// Already cooked bytes addressed by a logical project path.
#[derive(Debug, Clone)]
pub struct Input {
    /// Portable relative path.
    pub path: String,
    /// Cooked content, independent of the source root and timestamp.
    pub bytes: Vec<u8>,
}

/// A deterministic pack ready for atomic publication by the exporter.
pub struct BuiltPack {
    /// Complete immutable file bytes.
    pub bytes: Vec<u8>,
    /// The canonical manifest.
    pub manifest: Manifest,
}

/// Builds from cooked inputs. Fixed boundaries localize file edits and deduplicate
/// repeated blocks. All ordering comes from logical names and content hashes.
pub fn build(
    inputs: impl IntoIterator<Item = Input>,
    cook: CookKey,
    chunk_size: usize,
    key: Option<&EncryptionKey>,
) -> Result<BuiltPack, Error> {
    if !(1..=MAX_CHUNK).contains(&chunk_size) {
        return Err(invalid("chunk size must be 1..=16 MiB"));
    }
    let mut manifest = Manifest {
        format: "ara.pack/2".into(),
        cook,
        chunk_size: chunk_size as u32,
        encoding: if key.is_some() {
            "zstd-9+xchacha20poly1305"
        } else {
            "zstd-9"
        }
        .into(),
        entries: BTreeMap::new(),
        chunks: BTreeMap::new(),
    };
    let mut chunks = BTreeMap::<String, Vec<u8>>::new();
    for input in inputs {
        validate_path(&input.path)?;
        let mut ids = Vec::new();
        for raw in input.bytes.chunks(chunk_size) {
            let id = blake3::hash(raw).to_hex().to_string();
            if !chunks.contains_key(&id) {
                let compressed = zstd::bulk::compress(raw, 9)?;
                let stored = if let Some(key) = key {
                    XChaCha20Poly1305::new((&key.0).into())
                        .encrypt(
                            (&nonce(key, &id)).into(),
                            Payload {
                                msg: &compressed,
                                aad: id.as_bytes(),
                            },
                        )
                        .map_err(|_| Error::Key)?
                } else {
                    compressed
                };
                manifest.chunks.insert(
                    id.clone(),
                    Chunk {
                        offset: 0,
                        stored: stored.len() as u64,
                        raw: raw.len() as u64,
                        stored_hash: blake3::hash(&stored).to_hex().to_string(),
                    },
                );
                chunks.insert(id.clone(), stored);
            }
            ids.push(id);
        }
        let entry = Entry {
            hash: blake3::hash(&input.bytes).to_hex().to_string(),
            size: input.bytes.len() as u64,
            chunks: ids,
        };
        if manifest.entries.insert(input.path.clone(), entry).is_some() {
            return Err(invalid(format!("duplicate path {}", input.path)));
        }
    }
    let mut offset: usize = 0;
    for c in manifest.chunks.values_mut() {
        c.offset = offset as u64;
        offset = align(
            offset
                .checked_add(c.stored as usize)
                .ok_or_else(|| invalid("pack too large"))?,
        )?;
    }
    let json = canonical(&manifest)?;
    let data = align(
        HEADER_LEN
            .checked_add(json.len())
            .ok_or_else(|| invalid("manifest too large"))?,
    )?;
    let mut bytes = vec![
        0;
        data.checked_add(offset)
            .ok_or_else(|| invalid("pack too large"))?
    ];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&(json.len() as u64).to_le_bytes());
    bytes[24..32].copy_from_slice(&(data as u64).to_le_bytes());
    bytes[32..64].copy_from_slice(blake3::hash(&json).as_bytes());
    bytes[HEADER_LEN..HEADER_LEN + json.len()].copy_from_slice(&json);
    for (id, chunk) in chunks {
        let start = data + manifest.chunks[&id].offset as usize;
        bytes[start..start + chunk.len()].copy_from_slice(&chunk);
    }
    Ok(BuiltPack { bytes, manifest })
}

/// Cooks a directory using strict canonical JSON for `.json` and raw bytes for
/// media/already cooked assets. Symlinks are rejected so a moved source stays closed.
pub fn build_directory(
    root: &Path,
    cook: CookKey,
    chunk_size: usize,
    key: Option<&EncryptionKey>,
) -> Result<BuiltPack, Error> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<Input>) -> Result<(), Error> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_symlink() {
                return Err(invalid("pack source symlinks are not supported"));
            }
            if ty.is_dir() {
                walk(root, &entry.path(), out)?;
            } else if ty.is_file() {
                let path = entry.path();
                let name = path
                    .strip_prefix(root)
                    .map_err(|e| invalid(e.to_string()))?
                    .to_str()
                    .ok_or_else(|| invalid("non-UTF8 path"))?
                    .replace('\\', "/");
                let mut bytes = std::fs::read(&path)?;
                if path.extension().is_some_and(|x| x == "json") {
                    let text = std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?;
                    let value = ara_json::parse(text).map_err(|e| invalid(e.to_string()))?;
                    bytes =
                        ara_json::to_canonical_string(&ara_json::sort_keys(&value)).into_bytes();
                }
                out.push(Input { path: name, bytes });
            } else {
                return Err(invalid("pack source must contain regular files"));
            }
        }
        Ok(())
    }
    let mut inputs = Vec::new();
    walk(root, root, &mut inputs)?;
    build(inputs, cook, chunk_size, key)
}

fn align(n: usize) -> Result<usize, Error> {
    n.checked_add(ALIGN - 1)
        .map(|x| x / ALIGN * ALIGN)
        .ok_or_else(|| invalid("alignment overflow"))
}
