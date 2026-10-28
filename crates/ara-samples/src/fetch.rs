//! On-demand downloads: a content-addressed blob cache (`samples/.cache/blobs/<blake3>`),
//! resumable transfers (`curl -C -` into `.part` files) and integrity checks before a
//! blob is published. Project files are hard links (or copies) of verified blobs.
use crate::lock::{Asset, Lock};
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

#[cfg(test)]
mod tests;

/// The shared download cache under `samples/`.
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// The cache of the samples directory (`<samples>/.cache`).
    pub fn new(samples: &Path) -> Self {
        Self {
            root: samples.join(".cache"),
        }
    }

    /// Where a verified blob lives.
    pub fn blob(&self, hash: &str) -> PathBuf {
        self.root.join("blobs").join(hash)
    }

    /// Tries `url`, then `mirrors` in order, resuming each URL's own partial transfer.
    /// With an expected (BLAKE3, size), only matching bytes enter the blob cache.
    pub fn download(
        &self,
        url: &str,
        mirrors: &[String],
        expected: Option<(&str, u64)>,
    ) -> Result<(String, u64)> {
        if let Some((hash, size)) = expected {
            let blob = self.blob(hash);
            if blob.is_file() {
                if std::fs::metadata(&blob)?.len() == size && hash_file(&blob)? == hash {
                    return Ok((hash.to_owned(), size));
                }
                std::fs::remove_file(blob)?;
            }
        }
        let partial = self.root.join("partial");
        std::fs::create_dir_all(&partial)?;
        std::fs::create_dir_all(self.root.join("blobs"))?;
        let mut failures = Vec::new();
        for candidate in std::iter::once(url).chain(mirrors.iter().map(String::as_str)) {
            match self.download_from(candidate, &partial, expected) {
                Ok(result) => return Ok(result),
                Err(error) => failures.push(format!("{candidate}: {error:#}")),
            }
        }
        bail!("all download URLs failed:\n{}", failures.join("\n"))
    }

    /// Verifies one candidate before publishing it; incomplete transfers remain resumable.
    fn download_from(
        &self,
        url: &str,
        partial: &Path,
        expected: Option<(&str, u64)>,
    ) -> Result<(String, u64)> {
        let key = blake3::hash(url.as_bytes()).to_hex();
        let part = partial.join(format!("{}.part", &key[..32]));
        curl(url, &part)?;
        let hash = hash_file(&part)?;
        let size = std::fs::metadata(&part)?.len();
        if let Some((expected_hash, expected_size)) = expected {
            if hash != expected_hash {
                std::fs::remove_file(&part)?;
                bail!("hash mismatch: expected {expected_hash}, got {hash}");
            }
            if size != expected_size {
                std::fs::remove_file(&part)?;
                bail!("size mismatch: expected {expected_size}, got {size}");
            }
        }
        let blob = self.blob(&hash);
        if blob.is_file() && hash_file(&blob)? == hash {
            std::fs::remove_file(&part)?;
        } else {
            if blob.exists() {
                std::fs::remove_file(&blob)?;
            }
            std::fs::rename(&part, &blob)?;
        }
        Ok((hash, size))
    }

    /// Reads a small document (API JSON) without caching it.
    pub fn get_text(url: &str) -> Result<String> {
        let output = Command::new("curl")
            .args(["-fsSL", "--retry", "3", url])
            .output()
            .context("running curl (install it or put it on PATH)")?;
        ensure!(
            output.status.success(),
            "GET {url} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?)
    }
}

/// `curl` with retries and resume; the binary ships with Windows 10+, macOS and Linux.
fn curl(url: &str, out: &Path) -> Result<()> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--retry",
            "5",
            "--retry-delay",
            "2",
            "--write-out",
            "%{http_code}",
            "-C",
            "-",
            "-o",
        ])
        .arg(out)
        .arg(url)
        .output()
        .context("running curl (install it or put it on PATH)")?;
    // 33: the server refused the range because the partial file is already complete.
    ensure!(
        output.status.success() || output.status.code() == Some(33),
        "curl {} (HTTP {}): {}",
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// BLAKE3 of a file, lowercase hex.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut buffer = vec![0; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Fetches every asset of `projects` (all when empty) and materializes it into each
/// project that uses it. Already-present files are re-hashed, never re-downloaded.
pub fn fetch(samples: &Path, lock: &Lock, projects: &[String], jobs: usize) -> Result<()> {
    let cache = Cache::new(samples);
    let wanted: Vec<(&Asset, &str)> = lock
        .assets
        .iter()
        .flat_map(|a| a.projects.iter().map(move |p| (a, p.as_str())))
        .filter(|(_, p)| projects.is_empty() || projects.iter().any(|w| w == p))
        .collect();
    let queue = Mutex::new(wanted.iter());
    let errors = Mutex::new(Vec::new());
    let done = Mutex::new(0usize);
    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| {
                while let Some(&(asset, project)) = queue.lock().ok().and_then(|mut q| q.next()) {
                    if let Err(e) = materialize(&cache, &samples.join(project), asset)
                        && let Ok(mut errors) = errors.lock()
                    {
                        errors.push(format!("{project}/{}: {e:#}", asset.id));
                    }
                    if let Ok(mut done) = done.lock() {
                        *done += 1;
                        eprintln!("[{}/{}] {project}: {}", *done, wanted.len(), asset.id);
                    }
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap_or_default();
    if !errors.is_empty() {
        bail!("{} asset(s) failed:\n{}", errors.len(), errors.join("\n"));
    }
    Ok(())
}

/// Places every file of `asset` in `project`, downloading missing blobs.
fn materialize(cache: &Cache, project: &Path, asset: &Asset) -> Result<()> {
    ensure!(
        project.join("ara.project.json").is_file(),
        "{} is not an Ara project",
        project.display()
    );
    let dir = asset.dir(project);
    for file in &asset.files {
        let target = dir.join(&file.path);
        if target.is_file()
            && std::fs::metadata(&target)?.len() == file.size
            && hash_file(&target)? == file.blake3
        {
            continue;
        }
        let (hash, _) =
            cache.download(&file.url, &file.mirrors, Some((&file.blake3, file.size)))?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if target.exists() {
            std::fs::remove_file(&target)?;
        }
        let blob = cache.blob(&hash);
        if std::fs::hard_link(&blob, &target).is_err() {
            std::fs::copy(&blob, &target)?;
        }
    }
    if asset.kind == crate::lock::Kind::Archive {
        for file in &asset.files {
            extract(&dir.join(&file.path), &dir)?;
        }
    }
    Ok(())
}

/// ZIP paths never escape the verified pack's directory. Archives remain pinned
/// intact; extraction is deterministic and can be repeated after a damaged file.
fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let path = entry
            .enclosed_name()
            .context("archive contains an unsafe path")?;
        ensure!(
            entry
                .unix_mode()
                .is_none_or(|mode| mode & 0o170000 != 0o120000),
            "archive symlinks are not supported"
        );
        total = total
            .checked_add(entry.size())
            .context("archive size overflow")?;
        ensure!(total <= 512 * 1024 * 1024, "asset archive exceeds 512 MiB");
        let path = destination.join(path);
        if entry.is_dir() {
            std::fs::create_dir_all(path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if std::fs::read(&path).ok().as_deref() != Some(&bytes) {
            std::fs::write(path, bytes)?;
        }
    }
    Ok(())
}

/// Checks that every file of `projects` is present and verified, without downloading.
pub fn verify(samples: &Path, lock: &Lock, projects: &[String]) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    for asset in &lock.assets {
        for project in asset
            .projects
            .iter()
            .filter(|p| projects.is_empty() || projects.contains(p))
        {
            for file in &asset.files {
                let path = asset.dir(&samples.join(project)).join(&file.path);
                if !path.is_file()
                    || std::fs::metadata(&path)?.len() != file.size
                    || hash_file(&path)? != file.blake3
                {
                    missing.push(format!("{project}/{}", asset.relative(&file.path)));
                }
            }
        }
    }
    Ok(missing)
}
