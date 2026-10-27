//! DX replays workspace rustc directly, bypassing Cargo wrappers. Guard both routes.
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

pub(super) fn forward() -> Option<ExitCode> {
    let executable = std::env::current_exe().ok()?;
    if !matches!(
        executable.file_name()?.to_str(),
        Some("rustc" | "rustc.exe")
    ) {
        return None;
    }
    let compiler = std::env::var_os("ARA_HOTPATCH_RUSTC")?;
    Some(match compile(&compiler) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("ARA7602: {error:#}");
            ExitCode::FAILURE
        }
    })
}

fn compile(compiler: &std::ffi::OsStr) -> Result<u8> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let source = args
        .iter()
        .find(|arg| Path::new(arg).extension().is_some_and(|e| e == "rs"));
    let contract = if source.is_some() {
        let path = std::env::var_os("ARA_HOTPATCH_CONTRACT")
            .ok_or_else(|| anyhow::anyhow!("source contract is missing"))?;
        Some(ara_hotpatch::Contract::read(&PathBuf::from(path))?)
    } else {
        None
    };
    if let (Some(contract), Some(source)) = (&contract, source) {
        contract.validate_source(Path::new(source))?;
    }
    let status = Command::new(compiler).args(&args).status()?;
    if let (Some(contract), Some(source)) = (&contract, source) {
        contract.validate_source(Path::new(source))?;
    }
    Ok(status.code().unwrap_or(1) as u8)
}
