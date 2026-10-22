//! A named, pagefile-backed shared-memory section (Windows `CreateFileMappingW`). The host writes
//! a snapshot once; any number of players map it read-only by name. The POSIX equivalent is
//! `shm_open` + `mmap`; this spike only implements Windows.

/// A mapped view of a named section. Unmapped and closed on drop; the OS frees the section when
/// the last handle in any process closes.
pub struct SharedSection {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    view: windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}

#[cfg(windows)]
mod imp {
    use super::SharedSection;
    use anyhow::{Result, bail};
    use std::io;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP, FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile,
        OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
    };

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn map(handle: HANDLE, access: FILE_MAP, len: usize) -> Result<SharedSection> {
        // SAFETY: `handle` is a live section handle owned by the caller; mapping `len` bytes from
        // offset 0 is within the section, whose size is at least `len`.
        let view = unsafe { MapViewOfFile(handle, access, 0, 0, len) };
        if view.Value.is_null() {
            let err = io::Error::last_os_error();
            // SAFETY: `handle` is owned here and not used again.
            unsafe { CloseHandle(handle) };
            bail!("MapViewOfFile: {err}");
        }
        Ok(SharedSection { handle, view, len })
    }

    impl SharedSection {
        /// Creates section `name` holding a copy of `data`. Fails if the name already exists.
        pub fn create(name: &str, data: &[u8]) -> Result<Self> {
            let name = wide(name);
            let len = data.len().max(1);
            // SAFETY: INVALID_HANDLE_VALUE requests a pagefile-backed section; `name` is
            // NUL-terminated and outlives the call; null attributes mean default security.
            let handle = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE,
                    (len as u64 >> 32) as u32,
                    len as u32,
                    name.as_ptr(),
                )
            };
            if handle.is_null() {
                bail!("CreateFileMappingW: {}", io::Error::last_os_error());
            }
            // SAFETY: reads this thread's last-error value, set by the call above.
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                // SAFETY: `handle` is owned here and not used again.
                unsafe { CloseHandle(handle) };
                bail!("shared section name collision");
            }
            let section = map(handle, FILE_MAP_READ | FILE_MAP_WRITE, len)?;
            // SAFETY: the view is `len` writable bytes, exclusively ours until the name is
            // published to a player, and `data.len() <= len`.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    section.view.Value.cast::<u8>(),
                    data.len(),
                )
            };
            Ok(section)
        }

        /// Maps an existing section read-only.
        pub fn open(name: &str, len: usize) -> Result<Self> {
            let name = wide(name);
            // SAFETY: `name` is NUL-terminated and outlives the call.
            let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
            if handle.is_null() {
                bail!("OpenFileMappingW: {}", io::Error::last_os_error());
            }
            map(handle, FILE_MAP_READ, len.max(1))
        }

        /// The mapped bytes.
        pub fn bytes(&self) -> &[u8] {
            // SAFETY: the view maps at least `len` readable bytes and lives as long as `self`;
            // the writer only writes before publishing the name, so the bytes no longer change.
            unsafe { std::slice::from_raw_parts(self.view.Value.cast::<u8>(), self.len) }
        }
    }

    impl Drop for SharedSection {
        fn drop(&mut self) {
            // SAFETY: `view` and `handle` were created by `map` and are released exactly once.
            unsafe {
                UnmapViewOfFile(self.view);
                CloseHandle(self.handle);
            }
        }
    }

    // SAFETY: the section is plain shared memory with no thread affinity; access goes through
    // `&self`, and the bytes are immutable once published.
    unsafe impl Send for SharedSection {}
}

#[cfg(not(windows))]
impl SharedSection {
    pub fn create(_name: &str, _data: &[u8]) -> anyhow::Result<Self> {
        anyhow::bail!("the shm transport is Windows-only in this spike")
    }
    pub fn open(_name: &str, _len: usize) -> anyhow::Result<Self> {
        anyhow::bail!("the shm transport is Windows-only in this spike")
    }
    pub fn bytes(&self) -> &[u8] {
        &[]
    }
}
