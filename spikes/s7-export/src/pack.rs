//! `.arapack` v1: a deterministic, content-addressed content pack.
//!
//! Layout (little-endian):
//! - `0..64`: header. Magic `ARAPACK\0`, version `u32`, flags `u32` (0),
//!   manifest length `u64`, data offset `u64`, BLAKE3 of the manifest bytes.
//! - `64..`: the manifest. Canonical JSON: sorted keys, no whitespace.
//! - `data_offset..`: chunks, ordered by hash, each starting on a [`ALIGN`] boundary.
//!
//! Identity: the pack id is the manifest's BLAKE3. A chunk is named by the BLAKE3 of
//! its *cooked, uncompressed* bytes, so identical content is stored once and a store
//! delta between two builds only touches chunks that really changed.
//!
//! The reader borrows the whole file (`std::fs::read` on desktop, `AAsset_getBuffer`
//! on Android) and decodes with pure-Rust `ruzstd`, so the runtime template needs no
//! C toolchain per target. The writer lives behind the `tools` feature.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// File magic.
pub const MAGIC: [u8; 8] = *b"ARAPACK\0";
/// Format version this reader understands.
pub const VERSION: u32 = 1;
/// Chunk alignment: page-friendly for mmap and stable for store delta patching.
pub const ALIGN: u64 = 4096;
/// Fixed header size.
pub const HEADER_LEN: usize = 64;

/// The pack's table of contents.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    /// Always `ara.pack/1`.
    pub format: String,
    /// Cook profile the content was built for (`desktop`, `mobile`).
    pub profile: String,
    /// Logical path (what `@path` references resolve to) → chunk hash.
    pub entries: BTreeMap<String, String>,
    /// Chunk hash (hex BLAKE3 of cooked bytes) → placement.
    pub chunks: BTreeMap<String, ChunkInfo>,
}

/// Where a chunk lives and how it is stored.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChunkInfo {
    /// Byte offset relative to the data region.
    pub offset: u64,
    /// Stored (possibly compressed) size.
    pub size: u64,
    /// Cooked size after decompression.
    pub raw: u64,
    /// `zstd` or `none`.
    pub codec: String,
}

/// Why a pack could not be opened or read.
#[derive(Debug)]
pub enum PackError {
    /// The header is missing, has the wrong magic, or an unknown version.
    BadHeader(String),
    /// The manifest bytes do not hash to the header's manifest hash.
    ManifestHash,
    /// The manifest is not valid JSON for this format.
    Manifest(String),
    /// No entry with this logical path.
    Missing(String),
    /// A chunk range or codec is invalid, or decompression failed.
    Chunk(String),
    /// A chunk does not hash to its name.
    ChunkHash(String),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::BadHeader(m) => write!(f, "bad pack header: {m}"),
            PackError::ManifestHash => write!(f, "manifest hash mismatch (corrupt pack)"),
            PackError::Manifest(m) => write!(f, "bad manifest: {m}"),
            PackError::Missing(p) => write!(f, "no entry @{p} in pack"),
            PackError::Chunk(m) => write!(f, "bad chunk: {m}"),
            PackError::ChunkHash(h) => write!(f, "chunk {h} failed its hash check"),
        }
    }
}

impl std::error::Error for PackError {}

/// A pack opened over borrowed bytes.
pub struct Pack<'a> {
    bytes: &'a [u8],
    data_offset: u64,
    /// BLAKE3 of the manifest: the pack's identity.
    pub id: blake3::Hash,
    /// The decoded manifest.
    pub manifest: Manifest,
}

impl<'a> Pack<'a> {
    /// Validates the header and manifest hash and decodes the manifest. Chunks are read lazily.
    pub fn open(bytes: &'a [u8]) -> Result<Self, PackError> {
        let header = bytes
            .get(..HEADER_LEN)
            .ok_or_else(|| PackError::BadHeader("file shorter than header".into()))?;
        if header[..8] != MAGIC {
            return Err(PackError::BadHeader("wrong magic".into()));
        }
        let version = u32::from_le_bytes(take(header, 8));
        if version != VERSION {
            return Err(PackError::BadHeader(format!(
                "version {version}, expected {VERSION}"
            )));
        }
        let manifest_len = u64::from_le_bytes(take(header, 16));
        let data_offset = u64::from_le_bytes(take(header, 24));
        let manifest_hash: [u8; 32] = take(header, 32);
        let manifest_bytes = usize::try_from(manifest_len)
            .ok()
            .and_then(|len| bytes.get(HEADER_LEN..HEADER_LEN.checked_add(len)?))
            .ok_or_else(|| PackError::BadHeader("manifest out of range".into()))?;
        let id = blake3::hash(manifest_bytes);
        if id.as_bytes() != &manifest_hash {
            return Err(PackError::ManifestHash);
        }
        let manifest: Manifest = serde_json::from_slice(manifest_bytes)
            .map_err(|e| PackError::Manifest(e.to_string()))?;
        if manifest.format != "ara.pack/1" {
            return Err(PackError::Manifest(format!("format {}", manifest.format)));
        }
        Ok(Self {
            bytes,
            data_offset,
            id,
            manifest,
        })
    }

    /// Returns the cooked bytes of `path` (no leading `@`), decompressed and hash-verified.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, PackError> {
        let hash = self
            .manifest
            .entries
            .get(path)
            .ok_or_else(|| PackError::Missing(path.into()))?;
        let info =
            self.manifest.chunks.get(hash).ok_or_else(|| {
                PackError::Chunk(format!("entry @{path} names unknown chunk {hash}"))
            })?;
        let start = self.data_offset.checked_add(info.offset);
        let stored = start
            .and_then(|s| {
                Some((
                    usize::try_from(s).ok()?,
                    usize::try_from(s.checked_add(info.size)?).ok()?,
                ))
            })
            .and_then(|(s, e)| self.bytes.get(s..e))
            .ok_or_else(|| PackError::Chunk(format!("chunk {hash} out of range")))?;
        let raw_len =
            usize::try_from(info.raw).map_err(|_| PackError::Chunk("raw size overflow".into()))?;
        let raw = match info.codec.as_str() {
            "none" => stored.to_vec(),
            "zstd" => {
                let mut out = Vec::with_capacity(raw_len);
                ruzstd::decoding::FrameDecoder::new()
                    .decode_all_to_vec(stored, &mut out)
                    .map_err(|e| PackError::Chunk(format!("chunk {hash}: {e}")))?;
                out
            }
            other => return Err(PackError::Chunk(format!("unknown codec {other}"))),
        };
        if raw.len() != raw_len || blake3::hash(&raw).to_hex().as_str() != hash {
            return Err(PackError::ChunkHash(hash.clone()));
        }
        Ok(raw)
    }

    /// Total file size in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// True if the backing buffer is empty (never, for an opened pack).
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

fn take<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes[at..at + N]);
    out
}

/// A cooked texture: `ATEX`, width, height, format (1 = RGBA8, top-down), then pixels.
pub struct Texture<'a> {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA8 pixels, row-major, top row first.
    pub pixels: &'a [u8],
}

impl<'a> Texture<'a> {
    /// Parses a cooked `ATEX` blob.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, PackError> {
        let bad = || PackError::Chunk("not a cooked ATEX texture".into());
        if bytes.len() < 16 || &bytes[..4] != b"ATEX" {
            return Err(bad());
        }
        let width = u32::from_le_bytes(take(bytes, 4));
        let height = u32::from_le_bytes(take(bytes, 8));
        let format = u32::from_le_bytes(take(bytes, 12));
        let len = (width as usize) * (height as usize) * 4;
        if format != 1 || bytes.len() != 16 + len {
            return Err(bad());
        }
        Ok(Self {
            width,
            height,
            pixels: &bytes[16..],
        })
    }
}

/// Pack writer: cooks a source directory into a byte-deterministic `.arapack`.
#[cfg(feature = "tools")]
pub mod write {
    use std::collections::BTreeMap;
    use std::path::Path;

    use anyhow::{Context, Result, bail};

    use super::{ALIGN, ChunkInfo, HEADER_LEN, MAGIC, Manifest, VERSION};

    /// zstd level used for every chunk. Fixed, single-threaded: part of the determinism contract.
    pub const ZSTD_LEVEL: i32 = 19;

    /// Summary of one build.
    pub struct Built {
        /// The complete pack file.
        pub bytes: Vec<u8>,
        /// Pack id (manifest BLAKE3, hex).
        pub id: String,
        /// Number of logical entries.
        pub entries: usize,
        /// Number of unique chunks after dedupe.
        pub chunks: usize,
        /// Sum of cooked sizes.
        pub raw_bytes: u64,
    }

    /// Cooks every file under `src` for `profile` and returns the pack bytes.
    ///
    /// Determinism: inputs are sorted by normalized relative path; JSON is re-emitted in
    /// canonical form; no timestamps, absolute paths or host data enter the output.
    pub fn build(src: &Path, profile: &str) -> Result<Built> {
        let mut files = Vec::new();
        collect(src, src, &mut files)?;
        files.sort();

        let mut entries = BTreeMap::new();
        let mut cooked: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for rel in &files {
            let source = std::fs::read(src.join(rel)).with_context(|| format!("reading {rel}"))?;
            let bytes = cook(rel, &source).with_context(|| format!("cooking {rel}"))?;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            entries.insert(rel.clone(), hash.clone());
            cooked.entry(hash).or_insert(bytes);
        }

        let mut chunks = BTreeMap::new();
        let mut data = Vec::new();
        let mut raw_bytes = 0u64;
        for (hash, raw) in &cooked {
            pad_to(&mut data, ALIGN);
            let compressed = zstd::bulk::compress(raw, ZSTD_LEVEL).context("zstd")?;
            let (codec, stored) = if compressed.len() < raw.len() {
                ("zstd", compressed.as_slice())
            } else {
                ("none", raw.as_slice())
            };
            chunks.insert(
                hash.clone(),
                ChunkInfo {
                    offset: data.len() as u64,
                    size: stored.len() as u64,
                    raw: raw.len() as u64,
                    codec: codec.into(),
                },
            );
            data.extend_from_slice(stored);
            raw_bytes += raw.len() as u64;
        }

        let manifest = Manifest {
            format: "ara.pack/1".into(),
            profile: profile.into(),
            entries,
            chunks,
        };
        let manifest_bytes = canonical_json(&serde_json::to_value(&manifest)?)?;
        let manifest_hash = blake3::hash(&manifest_bytes);

        let mut out =
            Vec::with_capacity(HEADER_LEN + manifest_bytes.len() + data.len() + ALIGN as usize);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(manifest_bytes.len() as u64).to_le_bytes());
        let data_offset = (HEADER_LEN + manifest_bytes.len()).next_multiple_of(ALIGN as usize);
        out.extend_from_slice(&(data_offset as u64).to_le_bytes());
        out.extend_from_slice(manifest_hash.as_bytes());
        out.extend_from_slice(&manifest_bytes);
        out.resize(data_offset, 0);
        out.extend_from_slice(&data);

        Ok(Built {
            bytes: out,
            id: manifest_hash.to_hex().to_string(),
            entries: manifest.entries.len(),
            chunks: manifest.chunks.len(),
            raw_bytes,
        })
    }

    /// Serializes with sorted keys and no whitespace (serde_json's `Map` is a `BTreeMap`).
    pub fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(value)?)
    }

    fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
        for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                collect(root, &path, out)?;
            } else {
                let rel = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
        Ok(())
    }

    fn pad_to(buf: &mut Vec<u8>, align: u64) {
        let len = (buf.len() as u64).next_multiple_of(align);
        buf.resize(len as usize, 0);
    }

    /// Per-extension cook step. `.json` → canonical JSON; `.tga` → `ATEX` RGBA8; else raw.
    fn cook(rel: &str, source: &[u8]) -> Result<Vec<u8>> {
        if rel.ends_with(".json") {
            let value: serde_json::Value = serde_json::from_slice(source)?;
            return canonical_json(&value);
        }
        if rel.ends_with(".tga") {
            return cook_tga(source);
        }
        Ok(source.to_vec())
    }

    /// Uncompressed 32-bit truecolor TGA (type 2) → `ATEX` RGBA8, top row first.
    fn cook_tga(src: &[u8]) -> Result<Vec<u8>> {
        if src.len() < 18 || src[2] != 2 || src[16] != 32 {
            bail!("only uncompressed 32-bit TGA is supported");
        }
        let id_len = src[0] as usize;
        let width = u16::from_le_bytes([src[12], src[13]]) as usize;
        let height = u16::from_le_bytes([src[14], src[15]]) as usize;
        let top_down = src[17] & 0x20 != 0;
        let pixels = src
            .get(18 + id_len..18 + id_len + width * height * 4)
            .context("truncated TGA")?;
        let mut out = Vec::with_capacity(16 + pixels.len());
        out.extend_from_slice(b"ATEX");
        out.extend_from_slice(&(width as u32).to_le_bytes());
        out.extend_from_slice(&(height as u32).to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        for y in 0..height {
            let row = if top_down { y } else { height - 1 - y };
            for px in pixels[row * width * 4..(row + 1) * width * 4]
                .as_chunks::<4>()
                .0
            {
                out.extend_from_slice(&[px[2], px[1], px[0], px[3]]); // BGRA → RGBA
            }
        }
        Ok(out)
    }
}

#[cfg(all(test, feature = "tools"))]
mod tests {
    use super::{Pack, PackError, Texture, write};
    use std::path::Path;

    fn content() -> Vec<u8> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("content");
        write::build(&src, "desktop")
            .expect("content/ builds")
            .bytes
    }

    #[test]
    fn build_is_deterministic_and_round_trips() {
        let (a, b) = (content(), content());
        assert_eq!(a, b);
        let pack = Pack::open(&a).expect("opens");
        let scene: serde_json::Value =
            serde_json::from_slice(&pack.read("scene/main.scene.json").expect("scene"))
                .expect("json");
        assert_eq!(scene["format"], "ara.scene/0");
        let tex = pack.read("textures/checker.tga").expect("texture");
        let tex = Texture::parse(&tex).expect("cooked texture");
        assert_eq!((tex.width, tex.height), (64, 64));
    }

    #[test]
    fn corruption_is_detected() {
        let good = content();
        let pack = Pack::open(&good).expect("opens");
        let data_offset = u64::from_le_bytes(good[24..32].try_into().expect("8 bytes")) as usize;
        let first = pack
            .manifest
            .chunks
            .values()
            .map(|c| c.offset)
            .min()
            .expect("a chunk") as usize;

        let mut chunk_flip = good.clone();
        chunk_flip[data_offset + first + 8] ^= 0x40;
        let pack = Pack::open(&chunk_flip).expect("manifest untouched");
        let all = pack
            .manifest
            .entries
            .keys()
            .map(|p| pack.read(p))
            .collect::<Vec<_>>();
        assert!(
            all.iter()
                .any(|r| matches!(r, Err(PackError::ChunkHash(_)) | Err(PackError::Chunk(_))))
        );

        let mut manifest_flip = good;
        manifest_flip[70] ^= 0x01;
        assert!(matches!(
            Pack::open(&manifest_flip),
            Err(PackError::ManifestHash)
        ));
    }
}
