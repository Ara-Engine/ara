//! Fixed-metadata ZIP inputs and bounded package reads.
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};
/// Writes a stable ZIP with sorted paths, fixed timestamps, no host paths and no extras.
pub fn deterministic_zip(entries: &BTreeMap<String, Vec<u8>>, path: &Path) -> Result<()> {
    write_zip(entries, path, zip::CompressionMethod::Stored)
}
pub(super) fn write_zip(
    entries: &BTreeMap<String, Vec<u8>>,
    path: &Path,
    method: zip::CompressionMethod,
) -> Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path)?);
    for (name, data) in entries {
        ara_pack::validate_path(name)?;
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(method)
                .last_modified_time(zip::DateTime::default())
                .unix_permissions(0o644),
        )?;
        zip.write_all(data)?;
    }
    zip.finish()?;
    Ok(())
}

/// Reads regular ZIP entries, rejecting traversal, duplicates and excessive expansion.
pub fn zip_entries(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut out = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        ara_pack::validate_path(file.name())?;
        total = total
            .checked_add(file.size())
            .context("ZIP size overflow")?;
        ensure!(
            total <= 2 * 1024 * 1024 * 1024,
            "ARA-EXP-AND-023: ZIP exceeds 2 GiB"
        );
        let mut data = Vec::new();
        let name = file.name().to_owned();
        file.read_to_end(&mut data)?;
        ensure!(
            out.insert(name, data).is_none(),
            "ARA-EXP-AND-023: duplicate ZIP entry"
        );
    }
    Ok(out)
}
