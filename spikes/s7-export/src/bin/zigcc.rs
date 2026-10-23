//! `zigcc`: a `cc` stand-in that links Linux ELF binaries *on Windows* with `zig cc`
//! against a pinned glibc. Lets Windows export NativeAOT Linux images and the Rust
//! Linux template with no Linux machine.
//!
//! - NativeAOT: `dotnet publish -r linux-x64 -p:DisableUnsupportedError=true -p:CppCompilerAndLinker=zigcc -p:LinkerFlavor=lld`
//!   (with this exe's directory on PATH; MSBuild probes it with `where`).
//! - Rust: `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=<path>/zigcc.exe`.
//!
//! Target: `ARA_ZIG_TARGET` (default `x86_64-linux-gnu.2.31`, the Steam Runtime 3 "sniper"
//! baseline). Zig: `ARA_ZIG`, else `tools/zig-*/zig.exe` in the spike. Zig's cache goes to
//! `out/zig-cache` unless `ZIG_GLOBAL_CACHE_DIR` is set. Prior art: PublishAotCross.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = std::env::var("ARA_ZIG_TARGET").unwrap_or_else(|_| "x86_64-linux-gnu.2.31".into());
    let Some(zig) = std::env::var_os("ARA_ZIG")
        .map(PathBuf::from)
        .or_else(|| bundled_zig(&root))
    else {
        eprintln!("zigcc: zig not found (set ARA_ZIG or unpack zig into spikes/s7-export/tools/)");
        return ExitCode::from(127);
    };

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
    for arg in args {
        // The target comes from ARA_ZIG_TARGET; LLD is forced; zig has no compressed-debug flag
        // and warns that `-O1` is deprecated.
        if arg.starts_with("--target=")
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

fn bundled_zig(root: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(root.join("tools"))
        .ok()?
        .filter_map(|e| {
            e.ok()
                .map(|e| e.path().join(if cfg!(windows) { "zig.exe" } else { "zig" }))
        })
        .find(|p| p.is_file())
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
