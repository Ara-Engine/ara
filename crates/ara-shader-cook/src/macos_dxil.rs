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
    run(Command::new(dxc)
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
        .arg(output))?;
    std::fs::remove_file(hlsl)?;
    Ok(())
}
