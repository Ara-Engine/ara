//! dx 0.7.10's raw line-based linker capture needs Windows response-file quoting.
use anyhow::{Context, Result};
use std::{ffi::OsString, path::Path};

pub(super) fn prepare_argument(arg: OsString) -> Result<OsString> {
    let Some(file) = arg.to_str().and_then(|s| s.strip_prefix('@')) else {
        return Ok(arg);
    };
    let file = Path::new(file);
    let raw_capture = file
        .file_name()
        .is_some_and(|name| name == "link_args.json");
    let dx_response = file
        .file_name()
        .is_some_and(|name| name == "windows_command.txt");
    if !raw_capture && !dx_response {
        return Ok(arg);
    }
    // This pinned DX capture is raw argv joined by newlines, not a valid response
    // file. Leave rustc's own .rsp untouched. dx's quoted response also passes
    // through our __chkstk ABI workaround before it reaches the native linker.
    let raw = std::fs::read_to_string(file).context("read dx linker argument capture")?;
    let args: Vec<_> = if raw_capture {
        raw.lines().collect()
    } else {
        raw.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .context("unexpected dx Windows response format")?
            .split("\" \"")
            .collect()
    };
    let mut encoded = Vec::with_capacity(args.len());
    for argument in args {
        if empty_dx_archive(std::ffi::OsStr::new(argument)) {
            continue;
        }
        #[cfg(windows)]
        if Path::new(argument)
            .file_name()
            .is_some_and(|n| n == "stub.o")
        {
            let fixed = super::windows_stubs::prepare(Path::new(argument))?;
            encoded.push(quote(fixed.to_str().context("non-UTF-8 dx stub path")?));
            continue;
        }
        encoded.push(quote(argument));
    }
    let encoded = encoded.join("\n");
    let normalized = file.with_file_name(format!("ara-link-{}.rsp", std::process::id()));
    std::fs::write(&normalized, encoded).context("quote dx linker response")?;
    Ok(format!("@{}", normalized.display()).into())
}

/// DX writes a zero-byte libdeps archive when target outputs are outside its
/// checkout. Its first link skips it; cached links mistakenly force-load it.
/// The original compiler rlibs remain on the command line in both cases.
pub(super) fn empty_dx_archive(argument: &std::ffi::OsStr) -> bool {
    let Some(text) = argument.to_str() else {
        return false;
    };
    let path = Path::new(text.strip_prefix("/WHOLEARCHIVE:").unwrap_or(text));
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with("libdeps-") && name.ends_with(".a"))
        && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == 0)
}

// LLVM's Windows response parser follows the MSVC backslash-before-quote rules.
fn quote(arg: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            '\\',
            if ch == '"' { slashes * 2 + 1 } else { slashes },
        ));
        result.push(ch);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, process::Command};

    #[test]
    fn dx_raw_response_with_spaces_links_through_the_real_lld() {
        let scratch = tempfile::tempdir().unwrap();
        let source = scratch.path().join("entry.rs");
        let object = scratch.path().join("entry.obj");
        fs::write(
            &source,
            "#![no_std]\n#[unsafe(no_mangle)] pub extern \"C\" fn entry() -> i32 { 0 }\n",
        )
        .unwrap();
        let compiled = Command::new("rustc")
            .args(["--crate-type", "lib", "--emit=obj", "-Cpanic=abort"])
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let sysroot = Command::new("rustc")
            .args(["--print", "sysroot"])
            .output()
            .unwrap();
        let lld = PathBuf::from(String::from_utf8(sysroot.stdout).unwrap().trim())
            .join("lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe");
        let response = scratch.path().join("link_args.json");
        let executable = scratch.path().join("entry.exe");
        let empty = scratch.path().join("libdeps-test.a");
        fs::write(&empty, []).unwrap();
        fs::write(&response, format!("/NOLOGO\n/NODEFAULTLIB\n/ENTRY:entry\n/SUBSYSTEM:CONSOLE\n/OUT:{}\n/LIBPATH:{}\n/WHOLEARCHIVE:{}\n{}\n", executable.display(), scratch.path().join("SDK path with spaces").display(), empty.display(), object.display())).unwrap();
        let arg = OsString::from(format!("@{}", response.display()));
        let raw = Command::new(&lld)
            .args(["-flavor", "link"])
            .arg(&arg)
            .output()
            .unwrap();
        assert!(
            !raw.status.success(),
            "the raw dx response must reproduce the whitespace failure"
        );
        let fixed = Command::new(&lld)
            .args(["-flavor", "link"])
            .arg(prepare_argument(arg).unwrap())
            .output()
            .unwrap();
        assert!(
            fixed.status.success(),
            "{}",
            String::from_utf8_lossy(&fixed.stderr)
        );
        assert!(executable.is_file());
        super::super::linker::prepare_companion(&executable).unwrap();
        let image = fs::read(&executable).unwrap();
        let pe = u32::from_le_bytes(image[60..64].try_into().unwrap()) as usize;
        let stack = pe + 24 + 72;
        assert_eq!(
            u64::from_le_bytes(image[stack..stack + 8].try_into().unwrap()),
            16 * 1024 * 1024
        );
        fs::write(&empty, b"!<arch>\n").unwrap();
        assert!(!empty_dx_archive(empty.as_os_str()));
        let unrelated = scratch.path().join("custom.a");
        fs::write(&unrelated, []).unwrap();
        assert!(!empty_dx_archive(unrelated.as_os_str()));
    }
}
