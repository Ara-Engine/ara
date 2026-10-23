use anyhow::{Context, Result};
use ara_shader_cook::{Toolchain, cook};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let manifest = PathBuf::from(
        args.next()
            .context("Usage: ara-shader-cook <manifest.json> <output-dir> [sdk]")?,
    );
    let output = PathBuf::from(args.next().context("Missing output directory")?);
    let sdk = args
        .next()
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("VULKAN_SDK").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("C:/VulkanSDK/1.4.363.0"));
    let bin = sdk.join("Bin");
    let exe = |name: &str| bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let rebuilt = cook(
        &manifest,
        &output,
        &Toolchain {
            slangc: exe("slangc"),
            spirv_val: exe("spirv-val"),
        },
    )?;
    println!("Cooked {rebuilt} pipelines (unchanged artifacts reused)");
    Ok(())
}
