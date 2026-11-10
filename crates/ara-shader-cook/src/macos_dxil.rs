//! The SDK's Slang build cannot load DXC on macOS; use its HLSL output with DXC CLI.
use super::{Toolchain, run};
use anyhow::{Context, Result, ensure};
use ara_gpu::cooked::Stage;
use std::{path::Path, process::Command};

pub(super) fn compile(
    tools: &Toolchain,
    entry: &str,
    stage: Stage,
    hlsl: &Path,
    output: &Path,
) -> Result<()> {
    run(&mut dxc_command(tools, entry, stage, hlsl, output)?)?;
    std::fs::remove_file(hlsl)?;
    Ok(())
}

/// The DXC command that compiles Slang's HLSL for `entry` to DXIL at `output`, with the
/// flags the engine cook uses: the Vulkan SDK's `dxc` beside `slangc`. Other cooks that
/// run their stages side by side (the editor's material previews) spawn it themselves.
pub fn dxc_command(
    tools: &Toolchain,
    entry: &str,
    stage: Stage,
    hlsl: &Path,
    output: &Path,
) -> Result<Command> {
    let executable = tools.slangc.canonicalize().context("locating SDK Slang")?;
    let dxc = executable
        .parent()
        .context("Slang has no directory")?
        .join("dxc");
    ensure!(
        dxc.is_file(),
        "macOS DXIL cooking needs the Vulkan SDK's dxc beside slangc: {}",
        dxc.display()
    );
    let profile = match stage {
        Stage::Vertex => "vs_6_6",
        Stage::Fragment => "ps_6_6",
        Stage::Compute => "cs_6_6",
        Stage::Task => "as_6_6",
        Stage::Mesh => "ms_6_6",
    };
    let mut command = Command::new(dxc);
    command
        .arg(hlsl)
        .args([
            "-E",
            entry,
            "-T",
            profile,
            "-O2",
            "-Zpc",
            "-enable-16bit-types",
            "-HV",
            "2021",
            "-Fo",
        ])
        .arg(output);
    Ok(command)
}
