//! MSVC-shaped facade for dx, which cannot drive rust-lld's `-flavor link` entry.
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let Some(linker) = std::env::var_os("ARA_HOTPATCH_LLD") else {
        eprintln!("ARA_HOTPATCH_LLD must name the pinned toolchain's rust-lld.exe");
        return ExitCode::FAILURE;
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut command = Command::new(linker);
    if !args.iter().any(|arg| arg == "-flavor") {
        command.args(["-flavor", "link"]);
    }
    match command.args(args).status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!("hotpatch linker: {error}");
            ExitCode::FAILURE
        }
    }
}
