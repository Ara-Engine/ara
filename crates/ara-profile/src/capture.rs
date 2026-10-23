use std::{fs, io::{self, Read, Write}, path::Path};
use crate::{Profile, Summary, SummaryBudget};
const MAGIC: &[u8; 8] = b"ARAPRF01";
const MAX_BYTES: u64 = 512 * 1024 * 1024;

fn invalid(message: impl std::fmt::Display) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message.to_string()) }

/// Writes a versioned binary trace and sibling `.json` summary. File IO belongs
/// on a background/control thread; the render/tick thread only records events.
pub fn write_capture(path: &Path, profile: &Profile) -> io::Result<()> {
    let bytes = postcard::to_allocvec(profile).map_err(invalid)?;
    let mut file = fs::File::create(path)?;
    file.write_all(MAGIC)?; file.write_all(&(bytes.len() as u64).to_le_bytes())?; file.write_all(&bytes)?;
    write_summary(&path.with_extension("json"), &profile.summary(SummaryBudget { top_n: 1024, hitches: 1024, max_bytes: 1_048_576 }).map_err(invalid)?)
}
/// Reads a bounded binary trace, rejecting unsupported versions, truncation and trailing data.
pub fn read_capture(path: &Path) -> io::Result<Profile> {
    let mut file = fs::File::open(path)?; let mut magic = [0; 8]; file.read_exact(&mut magic)?;
    if &magic != MAGIC { return Err(invalid("unsupported Ara profile trace")); }
    let mut len = [0; 8]; file.read_exact(&mut len)?; let len = u64::from_le_bytes(len);
    if len > MAX_BYTES || file.metadata()?.len() != len + 16 { return Err(invalid("invalid profile length")); }
    let mut bytes = vec![0; len as usize]; file.read_exact(&mut bytes)?;
    let profile: Profile = postcard::from_bytes(&bytes).map_err(invalid)?;
    if profile.version != 1 { return Err(invalid("unsupported profile version")); }
    Ok(profile)
}
/// Saves a regression baseline through Ara's canonical JSON writer.
pub fn write_summary(path: &Path, summary: &Summary) -> io::Result<()> {
    let value = serde_json::to_value(summary).map_err(invalid)?;
    fs::write(path, ara_json::to_canonical_string(&value))
}
/// Reads a strict JSON baseline, rejecting duplicate keys and unsupported versions.
pub fn read_summary(path: &Path) -> io::Result<Summary> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_BYTES { return Err(invalid("summary is too large")); }
    let mut text = String::new(); file.take(MAX_BYTES + 1).read_to_string(&mut text)?;
    let summary: Summary = serde_json::from_value(ara_json::parse(&text).map_err(invalid)?).map_err(invalid)?;
    if summary.version != 1 { return Err(invalid("unsupported summary version")); }
    Ok(summary)
}
