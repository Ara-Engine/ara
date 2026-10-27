//! Keep normal builds on rustc's linker path; only dx uses this MSVC-shaped facade.
use anyhow::Result;
use std::{
    path::Path,
    process::{Command, ExitCode},
};

/// A copy named link.exe forwards raw linker arguments before clap sees them.
pub(crate) fn forward() -> Option<ExitCode> {
    let executable = std::env::current_exe().ok()?;
    if executable.file_name()? != "link.exe" {
        return None;
    }
    let linker = std::env::var_os("ARA_HOTPATCH_LLD")?;
    let args = match std::env::args_os()
        .skip(1)
        .map(super::response::prepare_argument)
        .collect::<Result<Vec<_>>>()
    {
        Ok(args) => args,
        Err(error) => {
            eprintln!("ARA7600: linker response: {error:#}");
            return Some(ExitCode::FAILURE);
        }
    };
    let mut command = Command::new(linker);
    if !args.iter().any(|a| a == "-flavor") {
        command.args(["-flavor", "link"]);
    }
    Some(match command.args(args).status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!("ARA7600: hotpatch rust-lld: {error}");
            ExitCode::FAILURE
        }
    })
}

pub(super) fn configure(root: &Path, command: &mut Command) -> Result<()> {
    let output = Command::new("rustc")
        .current_dir(root)
        .args(["--print", "sysroot"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "ARA7600: cannot locate the pinned Rust sysroot"
    );
    let sysroot = std::path::PathBuf::from(String::from_utf8(output.stdout)?.trim());
    // Stable compiler paths keep Cargo's warm artifacts across host restarts.
    // Key by tool bytes so another running host never has its facade overwritten.
    let executable = std::env::current_exe()?;
    let key = blake3::hash(&std::fs::read(&executable)?).to_hex();
    let tools = root.join("target/ara-hotpatch/tools").join(key.as_str());
    std::fs::create_dir_all(&tools)?;
    let directory = tools.as_path();
    let compiler = directory.join(if cfg!(windows) { "rustc.exe" } else { "rustc" });
    if !compiler.exists() {
        std::fs::copy(&executable, &compiler)?;
    }
    command
        .env(
            "ARA_HOTPATCH_RUSTC",
            sysroot
                .join("bin")
                .join(if cfg!(windows) { "rustc.exe" } else { "rustc" }),
        )
        .env("RUSTC", &compiler);
    // DX's direct replay uses PATH, Cargo uses RUSTC. Both reach the same guard.
    let mut paths = vec![directory.to_owned(), sysroot.join("bin")];
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(
            windows_sys::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS
                | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
        );
        let tool = cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
            .ok_or_else(|| {
                anyhow::anyhow!("ARA7600: MSVC/Windows SDK linker environment not found")
            })?;
        command.envs(tool.get_envs());
        // DX runs lld directly; unlike rustc it neither finds the sysroot nor supplies
        // `-flavor link`. Naming our facade link.exe also avoids rustc's lld hints.
        let facade = directory.join("link.exe");
        if !facade.exists() {
            std::fs::copy(&executable, &facade)?;
        }
        command
            .env(
                "ARA_HOTPATCH_LLD",
                sysroot.join("lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe"),
            )
            .env("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER", facade);
        if let Some((_, value)) = tool
            .get_envs()
            .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        {
            paths.extend(std::env::split_paths(value));
        }
    }
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env("PATH", std::env::join_paths(paths)?);
    Ok(())
}
