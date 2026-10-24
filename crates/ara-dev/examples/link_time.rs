//! Opt-in linker timing shim; see docs/dev/linking.md for repeatable measurements.
use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, ExitCode};
use std::time::Instant;

use anyhow::{Context, Result};

fn main() -> Result<ExitCode> {
    let linker = std::env::var_os("ARA_DEV_LINKER")
        .context("set ARA_DEV_LINKER to the real linker executable")?;
    let log = std::env::var_os("ARA_DEV_LINK_LOG")
        .context("set ARA_DEV_LINK_LOG to an output TSV under target/")?;
    let label = std::env::var("ARA_DEV_LINK_LABEL").unwrap_or_default();
    let mut command = Command::new(&linker);
    if std::path::Path::new(&linker)
        .file_stem()
        .is_some_and(|s| s == "rust-lld")
    {
        command.args(["-flavor", "link"]);
    }
    command.args(std::env::args_os().skip(1));
    let start = Instant::now();
    let status = command.status().context("run measured linker")?;
    let seconds = start.elapsed().as_secs_f64();
    let mut log = OpenOptions::new().create(true).append(true).open(log)?;
    writeln!(
        log,
        "{label}\t{}\t{seconds:.6}\t{}",
        linker.to_string_lossy(),
        status.code().unwrap_or(-1)
    )?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
