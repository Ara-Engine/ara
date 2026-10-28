//! Disk admission checks use the destination volume, including before it exists.
use crate::Error;
use std::{io, path::Path};

/// Space available to this user and bytes required by the next operation.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSpace {
    /// Free space on the destination volume.
    pub available_bytes: u64,
    /// Additional bytes required, excluding an already resumed download.
    pub required_bytes: u64,
}

/// Refuse before downloading or extracting when the destination cannot fit it.
pub fn check_space(destination: &Path, required_bytes: u64) -> Result<DiskSpace, Error> {
    let absolute = std::path::absolute(destination)?;
    let ancestor = absolute
        .ancestors()
        .find(|path| path.is_dir())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "destination has no existing directory",
            )
        })?;
    let space = DiskSpace {
        available_bytes: available(ancestor)?,
        required_bytes,
    };
    if space.required_bytes > space.available_bytes {
        return Err(Error::new(
            crate::IO,
            format!(
                "Not enough disk space at {}: {} bytes required, {} bytes available. Free space or choose another drive, then retry.",
                destination.display(),
                space.required_bytes,
                space.available_bytes
            ),
        ));
    }
    Ok(space)
}

#[cfg(windows)]
fn available(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0;
    // SAFETY: path is NUL-terminated and free points to writable storage; optional outputs are null.
    if unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

#[cfg(unix)]
fn available(path: &Path) -> io::Result<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in destination path"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated and stats is writable storage of the API's exact type.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful statvfs initialized the structure.
    let stats = unsafe { stats.assume_init() };
    // statvfs counters are platform-sized; widen them on 32-bit Unix as well.
    #[allow(clippy::unnecessary_cast)]
    Ok((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

#[cfg(not(any(windows, unix)))]
fn available(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "disk space queries are unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_missing_destination_on_its_existing_volume() {
        let root = crate::testing::scratch("disk-space");
        let missing = root.join("missing/nested");
        let space = check_space(&missing, 1).expect("volume query");
        assert!(space.available_bytes > 0);
        assert_eq!(space.required_bytes, 1);
        assert!(
            !missing.exists(),
            "preflight must not create the destination"
        );
        let error = check_space(&missing, u64::MAX).expect_err("insufficient space");
        assert_eq!(error.code, crate::IO);
        assert!(error.message.contains("retry"));
    }
}
