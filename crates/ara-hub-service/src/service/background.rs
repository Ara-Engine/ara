//! Detached user service startup must never inherit MCP or captured CLI pipe handles.
use std::{io, path::Path};

#[cfg(not(windows))]
pub(super) fn spawn(executable: &Path, directory: &Path) -> io::Result<()> {
    use std::{
        fs::File,
        process::{Command, Stdio},
    };
    let executable = std::path::absolute(executable)?;
    let directory = std::path::absolute(directory)?;
    Command::new(executable)
        .args(["hub", "serve", "--config-dir"])
        .arg(&directory)
        .current_dir(&directory)
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
    let executable = std::path::absolute(executable)?;
    let directory = std::path::absolute(directory)?;
    let working_directory: Vec<_> = directory.as_os_str().encode_wide().chain(Some(0)).collect();

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
    // environment pointer inherits only the environment; the explicit profile working
    // directory keeps the service from locking the GUI installation. FALSE forbids handles.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW,
            std::ptr::null(),
            working_directory.as_ptr(),
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::windows::process::CommandExt,
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    fn detached_service_uses_its_profile_not_the_launchers_directory() {
        let target = std::path::absolute(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/hub-startup-tests"),
        )
        .expect("target");
        fs::create_dir_all(&target).expect("target directory");
        let root = tempfile::tempdir_in(target).expect("isolated startup");
        let directory = root.path().join("profile");
        fs::create_dir(&directory).expect("profile");
        let script = root.path().join("fixture.ps1");
        let executable = root.path().join("ara-fixture.exe");
        fs::write(&script,r#"param($Output)
$ErrorActionPreference='Stop'
Add-Type -OutputAssembly $Output -OutputType ConsoleApplication -TypeDefinition 'public static class Fixture { public static int Main(string[] args) { var root=args[args.Length-1]; System.IO.File.WriteAllText(System.IO.Path.Combine(root,"cwd.tmp"),System.Environment.CurrentDirectory); System.IO.File.Move(System.IO.Path.Combine(root,"cwd.tmp"),System.IO.Path.Combine(root,"cwd.txt")); return 0; } }'
"#).expect("fixture source");
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script)
            .arg(&executable)
            .creation_flags(0x0800_0000)
            .output()
            .expect("compile fixture");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        spawn(&executable, &directory).expect("start detached service fixture");
        let ready = directory.join("cwd.txt");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.is_file() {
            assert!(
                Instant::now() < deadline,
                "service did not report its working directory"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let actual = fs::read_to_string(ready).expect("recorded cwd");
        assert_eq!(
            fs::canonicalize(actual).expect("actual directory"),
            fs::canonicalize(directory).expect("profile directory")
        );
    }
}
