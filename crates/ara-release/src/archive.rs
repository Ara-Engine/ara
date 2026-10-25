use anyhow::Result;
use std::{fs, io::Write, path::Path};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

/// Stable sorted file order and ZIP timestamps; callers select the payload files.
pub fn zip(source: &Path, destination: &Path) -> Result<()> {
    let mut archive = ZipWriter::new(fs::File::create(destination)?);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6));
    for file in crate::support::files(source)? {
        archive.start_file(
            file.strip_prefix(source)?
                .to_string_lossy()
                .replace('\\', "/"),
            options,
        )?;
        archive.write_all(&fs::read(file)?)?;
    }
    archive.finish()?.sync_all()?;
    Ok(())
}
