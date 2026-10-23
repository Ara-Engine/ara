//! `zigcc`: a `cc` stand-in that links Linux ELF binaries *on Windows* with `zig cc`
//! against a pinned glibc. Lets Windows export NativeAOT Linux images and the Rust
//! Linux template with no Linux machine.
//!
//! NativeAOT uses CppCompilerAndLinker=ara-zigcc; Rust uses
//! CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=<path>/ara-zigcc.exe.
//! ARA_ZIG must name Zig 0.17.0. The target is always x86_64-linux-gnu.2.31;
//! callers cannot override the baseline. ZIG_GLOBAL_CACHE_DIR can override cache placement.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let root = std::env::temp_dir().join("ara-zigcc");
    let target = "x86_64-linux-gnu.2.31".to_owned();
    let Some(zig) = std::env::var_os("ARA_ZIG")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    else {
        eprintln!("zigcc: ARA-EXP-LNX-006: set ARA_ZIG to Zig 0.17.0");
        return ExitCode::from(127);
    };

    let version = Command::new(&zig).arg("version").output();
    if !version
        .is_ok_and(|v| v.status.success() && String::from_utf8_lossy(&v.stdout).trim() == "0.17.0")
    {
        eprintln!("ARA-EXP-LNX-006: ara-zigcc requires Zig 0.17.0");
        return ExitCode::from(2);
    }
    let mut args = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.strip_prefix('@') {
            // MSBuild's link.rsp: one or more shell-quoted args per line.
            Some(file) => match std::fs::read_to_string(file) {
                Ok(text) => text.lines().for_each(|line| args.extend(split(line))),
                Err(e) => {
                    eprintln!("zigcc: reading {file}: {e}");
                    return ExitCode::from(2);
                }
            },
            None => args.push(arg),
        }
    }

    let mut out: Vec<String> = vec!["cc".into(), "-target".into(), target, "-fuse-ld=lld".into()];
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // The target is pinned; LLD is forced; Zig has no compressed-debug flag
        // and warns that `-O1` is deprecated.
        if matches!(arg.as_str(), "-target" | "--target" | "-fuse-ld") {
            if args.next().is_none() {
                eprintln!("ARA-EXP-LNX-008: linker target/flavor option has no value");
                return ExitCode::from(2);
            }
            continue;
        }
        if arg.starts_with("--target=")
            || arg.starts_with("-target=")
            || arg.starts_with("-fuse-ld=")
            || arg.starts_with("-gz")
            || arg == "-Wl,-O1"
        {
            continue;
        }
        out.push(arg);
    }

    let mut cmd = Command::new(zig);
    cmd.args(&out);
    if std::env::var_os("ZIG_GLOBAL_CACHE_DIR").is_none() {
        cmd.env("ZIG_GLOBAL_CACHE_DIR", root.join("out/zig-cache"));
    }
    if std::env::var_os("ARA_ZIGCC_TRACE").is_some() {
        eprintln!("zigcc: {cmd:?}");
    }
    match cmd.status() {
        Ok(s) => ExitCode::from(s.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("zigcc: running zig: {e}");
            ExitCode::from(126)
        }
    }
}

/// Splits one response-file line on whitespace outside double quotes; quotes are removed.
fn split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}
