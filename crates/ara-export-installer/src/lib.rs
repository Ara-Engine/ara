//! Bounded, validated extraction of an appended portable ZIP into a new directory.
//!
//! The payload is streamed from the installer file: [`Payload::open`] reads only the
//! PE header and trailer, [`Payload::verify`] hashes the archive with progress, and
//! only a [`Verified`] payload can list or extract entries into a [`Stage`].
#![warn(missing_docs)]
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, BufReader, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

/// Installer trailer magic. The trailer contains offset, length and BLAKE3.
pub const MAGIC: &[u8; 8] = b"ARASFX1\0";
const TRAILER: u64 = 56;
const CHUNK: usize = 1 << 20;

/// Appends a hash-checked archive to a prebuilt executable, before code signing.
pub fn append(stub: &[u8], archive: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        stub.starts_with(b"MZ"),
        "ARA-EXP-INSTALL-001: installer stub must be a Windows PE image"
    );
    ensure!(
        certificate_start(&mut Cursor::new(stub), stub.len() as u64).is_none(),
        "ARA-EXP-INSTALL-001: provide an unsigned installer stub; sign after appending the payload"
    );
    let mut bytes = stub.to_vec();
    let offset = bytes.len() as u64;
    bytes.extend_from_slice(archive);
    bytes.resize(bytes.len().div_ceil(8) * 8, 0);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&offset.to_le_bytes());
    bytes.extend_from_slice(&(archive.len() as u64).to_le_bytes());
    bytes.extend_from_slice(blake3::hash(archive).as_bytes());
    Ok(bytes)
}

fn read_at<const N: usize>(source: &mut (impl Read + Seek), offset: u64) -> Option<[u8; N]> {
    let mut bytes = [0; N];
    source.seek(SeekFrom::Start(offset)).ok()?;
    source.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

/// Start of an Authenticode certificate table that ends the file, if signed.
fn certificate_start(source: &mut (impl Read + Seek), len: u64) -> Option<u64> {
    let pe = u64::from(u32::from_le_bytes(read_at(source, 0x3c)?));
    if read_at::<4>(source, pe)? != *b"PE\0\0" {
        return None;
    }
    let opt = pe.checked_add(24)?;
    let dirs = match u16::from_le_bytes(read_at(source, opt)?) {
        0x20b => 112,
        0x10b => 96,
        _ => return None,
    };
    let security: [u8; 8] = read_at(source, opt.checked_add(dirs + 4 * 8)?)?;
    let offset = u64::from(u32::from_le_bytes(security[..4].try_into().ok()?));
    let size = u64::from(u32::from_le_bytes(security[4..].try_into().ok()?));
    (offset > 0 && offset.checked_add(size)? == len).then_some(offset)
}

/// A `Read + Seek` view of `len` bytes of `inner` starting at `start`. The inner
/// position is kept in sync, so reads never seek.
struct Window<R> {
    inner: R,
    start: u64,
    len: u64,
    pos: u64,
}
impl<R: Seek> Window<R> {
    fn new(mut inner: R, start: u64, len: u64) -> io::Result<Self> {
        inner.seek(SeekFrom::Start(start))?;
        Ok(Self {
            inner,
            start,
            len,
            pos: 0,
        })
    }
}
impl<R: Read> Read for Window<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let room = usize::try_from(self.len - self.pos)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let read = self.inner.read(&mut buf[..room])?;
        self.pos += read as u64;
        Ok(read)
    }
}
impl<R: Seek> Seek for Window<R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => self.len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        }
        .filter(|target| *target <= self.len)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek outside payload"))?;
        self.inner.seek(SeekFrom::Start(self.start + target))?;
        self.pos = target;
        Ok(target)
    }
}

/// An installer payload located by its trailer. Nothing inside the archive is
/// read until [`Payload::verify`] has checked its hash.
pub struct Payload<R> {
    source: R,
    start: u64,
    len: u64,
    hash: [u8; 32],
}
impl Payload<File> {
    /// Locates the payload appended to an installer executable on disk.
    pub fn open(path: &Path) -> Result<Self> {
        Self::new(File::open(path).with_context(|| format!("opening {}", path.display()))?)
    }
}
impl<R: Read + Seek> Payload<R> {
    /// Locates the trailer (honouring an Authenticode table) and checks the range.
    pub fn new(mut source: R) -> Result<Self> {
        let size = source.seek(SeekFrom::End(0))?;
        let end = certificate_start(&mut source, size).unwrap_or(size);
        let trailer_start = end
            .checked_sub(TRAILER)
            .context("ARA-EXP-INSTALL-001: missing installer trailer")?;
        let trailer: [u8; TRAILER as usize] = read_at(&mut source, trailer_start)
            .context("ARA-EXP-INSTALL-001: invalid installer trailer")?;
        ensure!(
            &trailer[..8] == MAGIC,
            "ARA-EXP-INSTALL-001: invalid installer trailer"
        );
        let start = u64::from_le_bytes(trailer[8..16].try_into()?);
        let len = u64::from_le_bytes(trailer[16..24].try_into()?);
        let finish = start
            .checked_add(len)
            .context("ARA-EXP-INSTALL-001: archive range overflow")?;
        ensure!(
            finish <= trailer_start && trailer_start - finish < 8,
            "ARA-EXP-INSTALL-001: invalid archive range"
        );
        Ok(Self {
            source,
            start,
            len,
            hash: trailer[24..].try_into()?,
        })
    }
    /// Archive size in bytes: the total that [`Payload::verify`] reports against.
    pub fn len(&self) -> u64 {
        self.len
    }
    /// True for an empty archive (always rejected by `verify`).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Hashes the archive against the trailer, reporting bytes read, then validates
    /// every entry name and declared size.
    pub fn verify(self, mut progress: impl FnMut(u64)) -> Result<Verified<R>> {
        let mut window = Window::new(self.source, self.start, self.len)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0; CHUNK];
        let mut done = 0;
        loop {
            let read = window.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            done += read as u64;
            progress(done);
        }
        ensure!(
            done == self.len,
            "ARA-EXP-INSTALL-001: truncated installer payload"
        );
        ensure!(
            hasher.finalize().as_bytes() == &self.hash,
            "ARA-EXP-INSTALL-001: corrupt installer payload"
        );
        let mut archive = zip::ZipArchive::new(BufReader::with_capacity(1 << 16, window))?;
        let (names, total) = validate(&mut archive)?;
        Ok(Verified {
            archive,
            names,
            total,
        })
    }
}

/// A hash-checked payload whose entry names and sizes passed validation.
pub struct Verified<R> {
    archive: zip::ZipArchive<BufReader<Window<R>>>,
    names: Vec<String>,
    total: u64,
}
impl<R: Read + Seek> Verified<R> {
    /// Validated entry names, in archive order.
    pub fn names(&self) -> &[String] {
        &self.names
    }
    /// Total expanded bytes: the total that [`Verified::extract`] reports against.
    pub fn total(&self) -> u64 {
        self.total
    }
    /// Writes and syncs every entry into `stage`, reporting the current entry and
    /// the expanded bytes written so far.
    pub fn extract(&mut self, stage: &Stage, mut progress: impl FnMut(&str, u64)) -> Result<()> {
        let mut buffer = vec![0; CHUNK];
        let mut done = 0;
        for name in &self.names {
            let mut input = self.archive.by_name(name)?;
            let path = stage.path().join(name);
            std::fs::create_dir_all(path.parent().context("file needs parent")?)?;
            let mut output = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)?;
            let size = input.size();
            let mut copied = 0;
            progress(name, done);
            loop {
                let read = input.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                copied += read as u64;
                ensure!(
                    copied <= size,
                    "ARA-EXP-INSTALL-003: invalid expanded file size"
                );
                output.write_all(&buffer[..read])?;
                done += read as u64;
                progress(name, done);
            }
            ensure!(
                copied == size,
                "ARA-EXP-INSTALL-003: invalid expanded file size"
            );
            output.flush()?;
            output.sync_all()?;
        }
        Ok(())
    }
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0'])
        && name.split('/').all(|part| {
            if part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part
                    .chars()
                    .any(|c| c.is_control() || "<>\"|?*".contains(c))
            {
                return false;
            }
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            !matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ) && !(stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        })
}

/// Validates every path and declared expansion; returns names and expanded total.
fn validate<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> Result<(Vec<String>, u64)> {
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let file = zip.by_index(i)?;
        ensure!(
            safe_name(file.name())
                && !file.is_dir()
                && file.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
            "ARA-EXP-INSTALL-002: unsafe archive entry"
        );
        ensure!(
            seen.insert(file.name().to_lowercase()),
            "ARA-EXP-INSTALL-002: duplicate archive path"
        );
        total = total
            .checked_add(file.size())
            .context("archive size overflow")?;
        ensure!(
            total <= 64 * 1024 * 1024 * 1024 && names.len() < 100_000,
            "ARA-EXP-INSTALL-003: archive expansion limit"
        );
        names.push(file.name().to_owned());
    }
    Ok((names, total))
}

/// Renames, retrying for about two seconds while Windows reports the source or
/// target in use (virus scanners briefly hold freshly written files).
pub fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
            Err(error) if attempt < 10 && matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(200));
            }
            result => return result,
        }
    }
}

/// A private sibling of the destination that receives extracted files. Dropping a
/// stage removes it; a successful [`Stage::rename_to`] hands the files over.
pub struct Stage {
    path: PathBuf,
    owned: bool,
}
impl Stage {
    /// Creates `.ara-install-<pid>-<nanos>` beside `destination`, creating parents.
    pub fn beside(destination: &Path) -> Result<Self> {
        let parent = destination
            .parent()
            .context("installation needs a parent directory")?;
        std::fs::create_dir_all(parent)?;
        let path = parent.join(format!(
            ".ara-install-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&path)?;
        Ok(Self { path, owned: true })
    }
    /// The staging directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Moves the stage to `destination`. On failure the stage is handed back,
    /// still cleaned up on drop.
    pub fn rename_to(mut self, destination: &Path) -> Result<(), (Self, io::Error)> {
        match rename_retrying(&self.path, destination) {
            Ok(()) => {
                self.owned = false;
                Ok(())
            }
            Err(error) => Err((self, error)),
        }
    }
    /// Publishes a new installation; an existing destination is never replaced.
    pub fn publish(self, destination: &Path) -> Result<()> {
        ensure!(
            !destination.exists(),
            "ARA-EXP-INSTALL-004: destination appeared during installation"
        );
        self.rename_to(destination).map_err(|(_, error)| error)?;
        Ok(())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Validates every path and declared expansion before returning the archive listing.
pub fn list(bytes: &[u8]) -> Result<Vec<String>> {
    let verified = Payload::new(Cursor::new(bytes))?.verify(|_| {})?;
    Ok(verified.names().to_vec())
}

/// Extracts and syncs into a private sibling, then publishes a new installation.
/// Existing destination directories are rejected, including empty directories.
pub fn install(bytes: &[u8], destination: &Path) -> Result<()> {
    let mut verified = Payload::new(Cursor::new(bytes))?.verify(|_| {})?;
    install_verified(&mut verified, destination)
}

/// [`install`] for an already verified payload, such as one streamed from disk.
pub fn install_verified<R: Read + Seek>(
    verified: &mut Verified<R>,
    destination: &Path,
) -> Result<()> {
    ensure!(
        !destination.exists(),
        "ARA-EXP-INSTALL-004: destination already exists"
    );
    let destination = std::path::absolute(destination)?;
    let stage = Stage::beside(&destination)?;
    verified.extract(&stage, |_, _| {})?;
    stage.publish(&destination)
}
