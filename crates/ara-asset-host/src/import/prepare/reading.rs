//! Guarded, cancellable transfers from explicitly authorized immutable sources.
use super::*;
use std::io::{Read, Write};

pub(super) fn copy(
    item: &external::Incoming,
    target: &Path,
    cancel: &Cancellation,
    progress: impl FnMut(u64),
) -> Result<Digest> {
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    transfer(item, &mut output, cancel, progress)?;
    drop(output);
    cancel.check()?;
    Ok(Digest::of(&fs::read(target)?))
}

pub(in crate::import) fn read_external(
    item: &external::Incoming,
    cancel: &Cancellation,
    progress: impl FnMut(u64),
) -> Result<Digest> {
    let mut bytes = Vec::new();
    transfer(item, &mut bytes, cancel, progress)?;
    cancel.check()?;
    Ok(Digest::of(&bytes))
}

/// Reads a prepared regular file through the same no-link/chunk/cancellation
/// guards as staging. Uses readonly source bytes in dry run and staged bytes in
/// real preparation; verifies the BLAKE3 proof before native/sidecar decoding.
pub fn read_entry(entry: &Entry, cancel: &Cancellation) -> Result<Vec<u8>> {
    cancel.check()?;
    if entry.kind != EntryKind::File {
        return Err(Error::Invalid(
            "only prepared regular files have decodable bytes".into(),
        ));
    }
    let path = entry
        .staged
        .as_ref()
        .or(entry.source.as_ref())
        .ok_or_else(|| Error::Invalid("prepared file has no authorized source".into()))?;
    external::no_links(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || external::linked(&metadata) || metadata.len() != entry.bytes {
        return Err(Error::Invalid(
            "prepared import input changed before decoding; retry".into(),
        ));
    }
    let item = external::Incoming {
        group: 0,
        source: path.clone(),
        relative: entry.source_reference.clone(),
        kind: EntryKind::File,
        bytes: entry.bytes,
        modified: metadata.modified().ok(),
    };
    let mut bytes = Vec::new();
    transfer(&item, &mut bytes, cancel, |_| {})?;
    if Some(Digest::of(&bytes)) != entry.digest {
        return Err(Error::Invalid(
            "prepared import input bytes changed before decoding; retry".into(),
        ));
    }
    Ok(bytes)
}

fn transfer(
    item: &external::Incoming,
    output: &mut impl Write,
    cancel: &Cancellation,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    external::no_links(&item.source)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x0002_0000);
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x0000_0100);
    }
    let mut source = options.open(&item.source)?;
    let before = source.metadata()?;
    if external::linked(&before)
        || !before.is_file()
        || before.len() != item.bytes
        || before.modified().ok() != item.modified
    {
        return Err(Error::Invalid(format!(
            "source changed during import preparation: {}; retry",
            item.source.display()
        )));
    }
    let mut buffer = [0u8; 64 * 1024];
    let mut length = 0;
    loop {
        cancel.check()?;
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > item.bytes {
            return Err(Error::Invalid(format!(
                "source grew while importing: {}; retry",
                item.source.display()
            )));
        }
        output.write_all(&buffer[..count])?;
        progress(count as u64);
    }
    let after = source.metadata()?;
    if length != item.bytes
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(Error::Invalid(format!(
            "source changed while importing: {}; retry",
            item.source.display()
        )));
    }
    cancel.check()?;
    Ok(())
}

pub(super) fn read(
    staged: &BTreeMap<PathBuf, (PathBuf, Digest)>,
    source: &Path,
    cancel: &Cancellation,
) -> Result<Vec<u8>> {
    cancel.check()?;
    let (path, digest) = staged.get(source).ok_or_else(|| {
        Error::Invalid(format!(
            "companion {} was not in the explicit drop snapshot; drop its containing folder",
            source.display()
        ))
    })?;
    let bytes = fs::read(path)?;
    if Digest::of(&bytes) != *digest {
        return Err(Error::Invalid(
            "prepared import input bytes changed; retry".into(),
        ));
    }
    cancel.check()?;
    Ok(bytes)
}
