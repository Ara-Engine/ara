//! Bounded, validated extraction of an appended portable ZIP into a new directory.
#![warn(missing_docs)]
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeSet,
    io::{Cursor, Read, Write},
    path::Path,
};

/// Installer trailer magic. The trailer contains offset, length and BLAKE3.
pub const MAGIC: &[u8; 8] = b"ARASFX1\0";
const TRAILER: usize = 56;

/// Appends an authenticated archive to a prebuilt executable, before code signing.
pub fn append(stub: &[u8], archive: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        stub.starts_with(b"MZ"),
        "ARA-EXP-INSTALL-001: installer stub must be a Windows PE image"
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

fn unsigned_end(bytes: &[u8]) -> Option<usize> {
    let pe = u32::from_le_bytes(bytes.get(0x3c..0x40)?.try_into().ok()?) as usize;
    if bytes.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let opt = pe.checked_add(24)?;
    let magic = u16::from_le_bytes(bytes.get(opt..opt + 2)?.try_into().ok()?);
    let dirs = match magic {
        0x20b => 112,
        0x10b => 96,
        _ => return None,
    };
    let security = opt.checked_add(dirs + 4 * 8)?;
    let offset = u32::from_le_bytes(bytes.get(security..security + 4)?.try_into().ok()?) as usize;
    let size = u32::from_le_bytes(bytes.get(security + 4..security + 8)?.try_into().ok()?) as usize;
    (offset > 0 && offset.checked_add(size)? == bytes.len()).then_some(offset)
}

fn archive(bytes: &[u8]) -> Result<&[u8]> {
    let end = unsigned_end(bytes).unwrap_or(bytes.len());
    let trailer = bytes
        .get(
            end.checked_sub(TRAILER)
                .context("missing installer trailer")?..end,
        )
        .context("invalid installer trailer")?;
    ensure!(
        &trailer[..8] == MAGIC,
        "ARA-EXP-INSTALL-001: invalid installer trailer"
    );
    let offset = usize::try_from(u64::from_le_bytes(trailer[8..16].try_into()?))?;
    let len = usize::try_from(u64::from_le_bytes(trailer[16..24].try_into()?))?;
    let finish = offset.checked_add(len).context("archive range overflow")?;
    ensure!(
        finish <= end - TRAILER && end - TRAILER - finish < 8,
        "ARA-EXP-INSTALL-001: invalid archive range"
    );
    let payload = bytes
        .get(offset..finish)
        .context("invalid archive offset")?;
    ensure!(
        blake3::hash(payload).as_bytes() == &trailer[24..],
        "ARA-EXP-INSTALL-001: corrupt installer payload"
    );
    Ok(payload)
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

/// Validates every path and declared expansion before returning the archive listing.
pub fn list(bytes: &[u8]) -> Result<Vec<String>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive(bytes)?))?;
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
    Ok(names)
}

/// Extracts and syncs into a private sibling, then publishes a new installation.
/// Existing destination directories are rejected, including empty directories.
pub fn install(bytes: &[u8], destination: &Path) -> Result<()> {
    let names = list(bytes)?;
    ensure!(
        !destination.exists(),
        "ARA-EXP-INSTALL-004: destination already exists"
    );
    let destination = std::path::absolute(destination)?;
    let parent = destination
        .parent()
        .context("installation needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let stage = parent.join(format!(
        ".ara-install-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&stage)?;
    let result = (|| {
        let mut zip = zip::ZipArchive::new(Cursor::new(archive(bytes)?))?;
        for name in names {
            let mut input = zip.by_name(&name)?;
            let path = stage.join(&name);
            std::fs::create_dir_all(path.parent().context("file needs parent")?)?;
            let mut output = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)?;
            let size = input.size();
            let copied = std::io::copy(&mut (&mut input).take(size + 1), &mut output)?;
            ensure!(
                copied == size,
                "ARA-EXP-INSTALL-003: invalid expanded file size"
            );
            output.flush()?;
            output.sync_all()?;
        }
        ensure!(
            !destination.exists(),
            "ARA-EXP-INSTALL-004: destination appeared during installation"
        );
        std::fs::rename(&stage, &destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    result
}
