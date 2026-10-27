use crate::{Error, Feed, HASH, MANIFEST, SIZE, VerifiedRelease, feed::check_status};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Hard ceiling independent of untrusted release metadata.
pub const MAX_DOWNLOAD: u64 = 16 * 1024 * 1024 * 1024;
const MAX_EXPANDED: u64 = 64 * 1024 * 1024 * 1024;
/// Transfer cancellation shared with its owner; no blocking lock.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);
impl Cancel {
    pub(crate) async fn cancelled(&self) -> Error {
        loop {
            if let Err(error) = self.check() {
                return error;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    /// Request cancellation at the next streaming chunk or extraction entry.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    /// Check at commit boundaries too, so cancelled work never registers an engine.
    pub fn check(&self) -> Result<(), Error> {
        if self.0.load(Ordering::Relaxed) {
            Err(Error::new(
                crate::CANCELLED,
                "download cancelled; partial data removed",
            ))
        } else {
            Ok(())
        }
    }
}
/// Signed byte count and current streamed position.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Bytes downloaded (including resumed data).
    pub received: u64,
    /// Verified manifest's exact size.
    pub total: u64,
}
/// Transfer stage, kept separate from byte progress so verification is visible.
#[derive(Clone, Copy, Debug)]
pub enum DownloadPhase {
    /// Receiving the authenticated archive, with resume when supported.
    Downloading,
    /// Checking every byte against the signed SHA-256 digest.
    Verifying,
}
/// Hash a file without reading the distribution into memory.
pub fn sha256_file(path: &Path) -> Result<String, Error> {
    digest_file(path, None)
}
fn digest_file(path: &Path, cancel: Option<&Cancel>) -> Result<String, Error> {
    let mut input = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        if let Some(cancel) = cancel {
            cancel.check()?;
        }
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(hex::encode(digest.finalize()))
}
/// Download only an authenticated archive. Network interruption keeps the partial file;
/// cancellation, tampering and oversize failures remove it. Resume identity is its signed hash.
pub fn download(
    feed: &Feed,
    release: &VerifiedRelease,
    stage: &Path,
    cancel: &Cancel,
    progress: impl FnMut(Progress),
) -> Result<PathBuf, Error> {
    download_with_phase(feed, release, stage, cancel, progress, |_| {})
}
/// Download with explicit stage notifications for user-visible install progress.
pub fn download_with_phase(
    feed: &Feed,
    release: &VerifiedRelease,
    stage: &Path,
    cancel: &Cancel,
    mut progress: impl FnMut(Progress),
    mut phase: impl FnMut(DownloadPhase),
) -> Result<PathBuf, Error> {
    let asset = release.archive()?;
    if asset.size > MAX_DOWNLOAD {
        return Err(Error::new(SIZE, "asset exceeds the 16 GiB download limit"));
    }
    fs::create_dir_all(stage)?;
    let partial = stage.join(format!("{}.part", asset.sha256));
    let result = (|| {
        cancel.check()?;
        let mut offset = fs::metadata(&partial).map_or(0, |m| m.len());
        if offset > asset.size {
            return Err(Error::new(
                SIZE,
                "partial download exceeds the signed length",
            ));
        }
        crate::check_space(stage, asset.size - offset)?;
        phase(DownloadPhase::Downloading);
        if offset < asset.size {
            let url = feed.asset_url(asset)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
            let mut response = feed.request_download(&url, (offset > 0).then_some(offset), cancel).await?;
            check_status(response.status(), response.headers())?;
            if response.status().as_u16() == 206 {
                let expected = format!("bytes {offset}-{}/{}", asset.size - 1, asset.size);
                if response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|h| h.to_str().ok())
                    != Some(expected.as_str())
                {
                    return Err(Error::new(
                        MANIFEST,
                        "resume returned an unexpected Content-Range",
                    ));
                }
            } else if response.status().as_u16() == 200 {
                offset = 0;
            } else {
                return Err(Error::new(MANIFEST, "download requires HTTP 200 or 206"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > asset.size - offset)
            {
                return Err(Error::new(SIZE, "asset exceeds the signed length"));
            }
            let mut output = OpenOptions::new()
                .create(true)
                .write(true)
                .append(offset > 0)
                .truncate(offset == 0)
                .open(&partial)?;
            progress(Progress {
                received: offset,
                total: asset.size,
            });
            loop {
                cancel.check()?;
                let chunk = tokio::select! {
                    chunk = response.chunk() => chunk.map_err(|_| Error::new(crate::NETWORK, "download interrupted; retry to resume"))?,
                    error = cancel.cancelled() => return Err(error),
                };
                let Some(chunk) = chunk else { break; };
                offset += chunk.len() as u64;
                if offset > asset.size {
                    return Err(Error::new(SIZE, "asset exceeds the signed length"));
                }
                output.write_all(&chunk)?;
                progress(Progress {
                    received: offset,
                    total: asset.size,
                });
            }
            output.sync_all()?;
            Ok::<(), Error>(())
            })?;
        }
        if offset != asset.size {
            return Err(Error::new(
                crate::NETWORK,
                "download incomplete; retry to resume",
            ));
        }
        phase(DownloadPhase::Verifying);
        if digest_file(&partial, Some(cancel))? != asset.sha256 {
            return Err(Error::new(
                HASH,
                "download SHA-256 differs from the signed manifest",
            ));
        }
        cancel.check()?;
        Ok(partial.clone())
    })();
    if result
        .as_ref()
        .is_err_and(|e: &Error| !matches!(e.code, crate::NETWORK | crate::RATE_LIMIT | crate::IO))
    {
        let _ = fs::remove_file(&partial);
    }
    result
}
/// Extract an archive into a new staging directory. Reject traversal, links, Windows
/// alternate streams/device names, duplicate files and both declared/actual expansion bombs.
pub fn extract(archive: &Path, destination: &Path, cancel: &Cancel) -> Result<(), Error> {
    // Query the ZIP only after its download checksum has been verified. Check the
    // exact expanded requirement before creating any installation directory.
    let mut zip = zip::ZipArchive::new(File::open(archive)?)
        .map_err(|e| Error::new(MANIFEST, e.to_string()))?;
    let mut required = 0_u64;
    for index in 0..zip.len() {
        cancel.check()?;
        required = required
            .checked_add(
                zip.by_index(index)
                    .map_err(|e| Error::new(MANIFEST, e.to_string()))?
                    .size(),
            )
            .ok_or_else(|| Error::new(SIZE, "archive size overflow"))?;
        if required > MAX_EXPANDED {
            return Err(Error::new(SIZE, "expanded archive exceeds 64 GiB limit"));
        }
    }
    crate::check_space(destination, required)?;
    fs::create_dir(destination)?;
    let result = (|| {
        if zip.len() > 200_000 {
            return Err(Error::new(SIZE, "too many archive entries"));
        }
        let mut total = 0_u64;
        let mut names = std::collections::BTreeSet::new();
        for i in 0..zip.len() {
            cancel.check()?;
            let mut entry = zip
                .by_index(i)
                .map_err(|e| Error::new(MANIFEST, e.to_string()))?;
            let raw = entry.name().replace('\\', "/");
            let path = Path::new(&raw);
            if raw.starts_with('/')
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || path.components().any(|c| {
                    let text = c.as_os_str().to_string_lossy();
                    let stem = text
                        .split('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase();
                    text.contains(':')
                        || text.ends_with([' ', '.'])
                        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                        || (stem.len() == 4
                            && (stem.starts_with("COM") || stem.starts_with("LPT"))
                            && stem.ends_with(|c: char| c.is_ascii_digit()))
                })
                || entry
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
                || !names.insert(raw.trim_end_matches('/').to_ascii_lowercase())
            {
                return Err(Error::new(MANIFEST, "unsafe or duplicate archive path"));
            }
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| Error::new(SIZE, "archive size overflow"))?;
            if total > MAX_EXPANDED {
                return Err(Error::new(SIZE, "expanded archive exceeds 64 GiB limit"));
            }
            let target = destination.join(path);
            if entry.is_dir() {
                fs::create_dir_all(target)?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let size = entry.size();
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(target)?;
                let mut actual = 0_u64;
                let mut buffer = [0; 64 * 1024];
                loop {
                    cancel.check()?;
                    let n = entry.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    actual += n as u64;
                    if actual > size {
                        return Err(Error::new(SIZE, "archive entry exceeds declared size"));
                    }
                    output.write_all(&buffer[..n])?;
                }
                if actual != size {
                    return Err(Error::new(SIZE, "archive entry differs from declared size"));
                }
                output.sync_all()?;
            }
        }
        cancel.check()
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}
