//! Atomically create a private directory even beneath the shared Windows temp root.
use std::{io, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::CreateDirectoryW,
};

pub(super) fn private_directory(path: &Path) -> io::Result<()> {
    // Protected DACL: only the owner has access; child sockets inherit the rule.
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)\0".encode_utf16().collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: the terminated SDDL and writable output pointer live through the call.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: the terminated path and valid security descriptor live through the call.
    let created = unsafe { CreateDirectoryW(path.as_ptr(), &attributes) };
    let result = if created == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    // SAFETY: the successful conversion allocated this descriptor with LocalAlloc.
    unsafe { LocalFree(descriptor) };
    result
}
