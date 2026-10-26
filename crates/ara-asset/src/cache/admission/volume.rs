use std::{io, path::Path};

/// Resolve the destination volume, including Windows directory mount points.
#[cfg(windows)]
pub(super) fn identity(root: &Path) -> io::Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    let path: Vec<_> = std::path::absolute(root)?
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    let mut mount = vec![0u16; 32768];
    // SAFETY: NUL-terminated input and writable output with its exact capacity.
    if unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut guid = [0u16; 64];
    // SAFETY: The previous call terminated mount; guid is a valid output buffer.
    let local = unsafe {
        GetVolumeNameForVolumeMountPointW(mount.as_ptr(), guid.as_mut_ptr(), guid.len() as u32)
    } != 0;
    // Network shares may not have a local volume GUID. Their share root is the
    // conservative I/O domain; different projects on that share still coordinate.
    let name = if local { &guid[..] } else { &mount[..] };
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Ok(String::from_utf16_lossy(&name[..end]).to_ascii_uppercase())
}

#[cfg(unix)]
pub(super) fn identity(root: &Path) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("device-{}", std::fs::metadata(root)?.dev()))
}

#[cfg(not(any(windows, unix)))]
pub(super) fn identity(root: &Path) -> io::Result<String> {
    Ok(std::fs::canonicalize(root)?.display().to_string())
}
