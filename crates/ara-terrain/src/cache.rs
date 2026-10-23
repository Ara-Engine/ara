//! Immutable binary cache access. Reads/cooking belong on workers, never frame code.
use crate::{Tile, invalid};
use ara_diag::Diagnostic;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Validates a lowercase BLAKE3 content key before using it as a filename.
pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Immutable binary lookup used by tile/stamp compilation.
pub trait TileSource {
    /// Read and verify an immutable tile by content hash.
    fn load(&self, hash: &str) -> Result<Tile, Diagnostic>;
}

/// In-memory cache for tests, importers and worker handoffs.
#[derive(Default)]
pub struct MemoryCache(BTreeMap<String, Vec<u8>>);
impl MemoryCache {
    /// Stores a derived tile and returns its immutable content key.
    pub fn put(&mut self, tile: &Tile) -> Result<String, Diagnostic> {
        let bytes = tile.encode()?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        self.0.insert(hash.clone(), bytes);
        Ok(hash)
    }
}
impl TileSource for MemoryCache {
    fn load(&self, hash: &str) -> Result<Tile, Diagnostic> {
        let bytes = self
            .0
            .get(hash)
            .ok_or_else(|| invalid("tile payload is not resident in cache"))?;
        Tile::decode(bytes)
    }
}

/// Filesystem content-addressed cache. Construct/use only on worker paths.
pub struct FileCache {
    root: PathBuf,
}
impl FileCache {
    /// Selects the derived-data directory without performing I/O.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn path(&self, hash: &str) -> Result<PathBuf, Diagnostic> {
        if !valid_hash(hash) {
            return Err(invalid("invalid cache hash"));
        }
        Ok(self.root.join(&hash[..2]).join(format!("{hash}.arth")))
    }
    /// Writes a complete immutable tile with create-new semantics; existing data is verified.
    pub fn put(&self, tile: &Tile) -> Result<String, Diagnostic> {
        use std::io::Write;
        let bytes = tile.encode()?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let path = self.path(&hash)?;
        if path.exists() {
            self.load(&hash)?;
            return Ok(hash);
        }
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| invalid("cache has no parent"))?,
        )
        .map_err(io)?;
        // Publish via a complete temporary file; readers never see partial final payloads.
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        match fs::rename(&temp, &path) {
            Ok(()) => {}
            Err(_) if path.exists() => {
                fs::remove_file(&temp).map_err(io)?;
                self.load(&hash)?;
            }
            Err(e) => {
                let _ = fs::remove_file(&temp);
                return Err(io(e));
            }
        }
        Ok(hash)
    }
    /// Cache directory used by an out-of-process tile cooker.
    pub fn root(&self) -> &Path {
        &self.root
    }
}
fn io(e: std::io::Error) -> Diagnostic {
    invalid(e.to_string())
}
impl TileSource for FileCache {
    fn load(&self, hash: &str) -> Result<Tile, Diagnostic> {
        let path = self.path(hash)?;
        if fs::metadata(&path).map_err(io)?.len() > 16 + 257 * 257 * 9 {
            return Err(invalid("oversized tile payload"));
        }
        let bytes = fs::read(path).map_err(io)?;
        if blake3::hash(&bytes).to_hex().as_str() != hash {
            return Err(invalid("tile content hash mismatch"));
        }
        Tile::decode(&bytes)
    }
}
