//! Named shared-memory sections for snapshot transfer (S5: ~0.1 ms for 0.5 MB, and N
//! players map one encoded snapshot). The owner creates a section and keeps it alive
//! for the session; players map it read-only by name. Names should embed part of the
//! session token: any same-session process can open a `Local\` name.
//!
//! Windows uses pagefile-backed sections. Other platforms report `Unsupported`; the
//! owner then sends [`crate::SnapshotRef::Inline`] bytes instead.

/// A mapped section; unmapped and closed on drop.
pub struct Section {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    view: windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}

// SAFETY: the section owns its kernel handle and mapping; the view is only read
// through `bytes(&self)` after creation finished writing it, so sharing is sound.
unsafe impl Send for Section {}
// SAFETY: see `Send`; no interior mutation happens after construction.
unsafe impl Sync for Section {}

/// Shared-memory failures, with the OS error code when relevant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShmError(pub String);

impl std::fmt::Display for ShmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ShmError {}

#[cfg(windows)]
fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Section {
    /// Creates a section named `name` holding a copy of `bytes`.
    #[cfg(windows)]
    pub fn create(name: &str, bytes: &[u8]) -> Result<Self, ShmError> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE},
            System::Memory::{CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE},
        };
        let len = bytes.len().max(1);
        let name = wide(name);
        // SAFETY: a pagefile-backed mapping of `len` bytes; the name is NUL-terminated.
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
            // SAFETY: reads the calling thread's last error.
            return Err(ShmError(format!("CreateFileMappingW failed: {}", unsafe {
                GetLastError()
            })));
        }
        // SAFETY: maps the whole section we just created with write access.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_WRITE, 0, 0, len) };
        if view.Value.is_null() {
            // SAFETY: the handle is ours and not used again.
            unsafe { CloseHandle(handle) };
            return Err(ShmError("MapViewOfFile failed".into()));
        }
        // SAFETY: the view is `len >= bytes.len()` writable bytes we exclusively own.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.Value.cast::<u8>(), bytes.len())
        };
        Ok(Self {
            handle,
            view,
            len: bytes.len(),
        })
    }

    /// Maps an existing section read-only; `len` is the payload length announced by
    /// the owner.
    #[cfg(windows)]
    pub fn open(name: &str, len: usize) -> Result<Self, ShmError> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, GetLastError},
            System::Memory::{FILE_MAP_READ, MapViewOfFile, OpenFileMappingW},
        };
        let name = wide(name);
        // SAFETY: opens a named mapping read-only; the name is NUL-terminated.
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
        if handle.is_null() {
            // SAFETY: reads the calling thread's last error.
            return Err(ShmError(format!("OpenFileMappingW failed: {}", unsafe {
                GetLastError()
            })));
        }
        // SAFETY: maps `len` bytes read-only; the owner created at least that many.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, len.max(1)) };
        if view.Value.is_null() {
            // SAFETY: the handle is ours and not used again.
            unsafe { CloseHandle(handle) };
            return Err(ShmError(
                "MapViewOfFile failed (section smaller than announced?)".into(),
            ));
        }
        Ok(Self { handle, view, len })
    }

    /// Sections are Windows-only for now.
    #[cfg(not(windows))]
    pub fn create(_: &str, _: &[u8]) -> Result<Self, ShmError> {
        Err(ShmError(
            "Unsupported: shared-memory sections are Windows-only; send inline bytes".into(),
        ))
    }

    /// Sections are Windows-only for now.
    #[cfg(not(windows))]
    pub fn open(_: &str, _: usize) -> Result<Self, ShmError> {
        Err(ShmError(
            "Unsupported: shared-memory sections are Windows-only".into(),
        ))
    }

    /// The mapped payload.
    pub fn bytes(&self) -> &[u8] {
        #[cfg(windows)]
        {
            // SAFETY: the view maps at least `len` bytes for the lifetime of `self`.
            unsafe { std::slice::from_raw_parts(self.view.Value.cast::<u8>(), self.len) }
        }
        #[cfg(not(windows))]
        {
            &[]
        }
    }
}

#[cfg(windows)]
impl Drop for Section {
    fn drop(&mut self) {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Memory::UnmapViewOfFile};
        // SAFETY: the view and handle were created by this object and are released once.
        unsafe {
            UnmapViewOfFile(self.view);
            CloseHandle(self.handle);
        }
    }
}

#[cfg(all(test, windows))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_section_is_visible_by_name() {
        let name = format!("Local\\ara-play-test-{}", std::process::id());
        let owner = Section::create(&name, b"snapshot bytes").unwrap();
        let reader = Section::open(&name, 14).unwrap();
        assert_eq!(reader.bytes(), b"snapshot bytes");
        assert_eq!(owner.bytes(), reader.bytes());
        assert!(Section::open("Local\\ara-play-missing", 4).is_err());
    }
}
