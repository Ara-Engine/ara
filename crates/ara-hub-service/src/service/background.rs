//! Detached user service startup must never inherit MCP or captured CLI pipe handles.
use std::{io, path::Path};

#[cfg(not(windows))]
pub(super) fn spawn(executable: &Path, directory: &Path) -> io::Result<()> {
    use std::{
        fs::File,
        process::{Command, Stdio},
    };
    Command::new(executable)
        .args(["hub", "serve", "--config-dir"])
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(directory.join("hub-start.log"))?)
        .spawn()
        .map(|_| ())
}

#[cfg(windows)]
pub(super) fn spawn(executable: &Path, directory: &Path) -> io::Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW},
    };

    // Quote using CommandLineToArgvW/CRT rules. This is a command line, never a shell script.
    fn argument(value: &OsStr) -> Vec<u16> {
        let mut output = vec![b'"' as u16];
        let mut slashes = 0;
        for c in value.encode_wide() {
            if c == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            output.extend(std::iter::repeat_n(
                b'\\' as u16,
                if c == b'"' as u16 {
                    slashes * 2 + 1
                } else {
                    slashes
                },
            ));
            slashes = 0;
            output.push(c);
        }
        output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        output.push(b'"' as u16);
        output
    }
    let application: Vec<_> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut command = Vec::new();
    for arg in [
        executable.as_os_str(),
        OsStr::new("hub"),
        OsStr::new("serve"),
        OsStr::new("--config-dir"),
        directory.as_os_str(),
    ] {
        if !command.is_empty() {
            command.push(b' ' as u16);
        }
        command.extend(argument(arg));
    }
    command.push(0);
    // SAFETY: Both Windows POD structs permit zero initialization; cb is set before use.
    let (mut startup, mut process): (STARTUPINFOW, PROCESS_INFORMATION) =
        unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    // SAFETY: Buffers are NUL-terminated UTF-16 and live for this call. Null optional
    // pointers inherit the environment/directory; FALSE explicitly forbids inheriting handles.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateProcessW succeeded, transferring two valid handles to us. The
    // detached service owns its lifetime; close only our handles, not the process.
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}
