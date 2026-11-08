//! Full logical state, independent of either engine's physical component layout.
use std::io::Write;
use std::path::Path;

/// A query-filter record: ID, tags, value, epoch, A/B/C, updates, buffer length.
#[derive(Clone, Copy, Default)]
pub struct Record(pub [u32; 9]);

/// Emits every authoritative field and returns the byte length and BLAKE3 digest.
pub fn write(path: &Path, records: &[Record], steps: u32) -> std::io::Result<(u64, String)> {
    let mut file = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?,
    );
    let mut hash = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut append = |value: &[u8]| -> std::io::Result<()> {
        file.write_all(value)?;
        hash.update(value);
        bytes += value.len() as u64;
        Ok(())
    };
    append(b"ARAECSF1")?;
    for value in [1_u32, 1, records.len() as u32, steps] {
        append(&value.to_le_bytes())?;
    }
    for record in records {
        for value in record.0 {
            append(&value.to_le_bytes())?;
        }
    }
    file.flush()?;
    Ok((bytes, hash.finalize().to_hex().to_string()))
}
