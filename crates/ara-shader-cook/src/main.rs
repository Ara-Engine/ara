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
    let tools = if let Some(sdk) = args.next() {
        let bin = PathBuf::from(sdk).join(if cfg!(windows) { "Bin" } else { "bin" });
        let exe = |name: &str| bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        Toolchain {
            slangc: exe("slangc"),
            spirv_val: exe("spirv-val"),
        }
    } else {
        Toolchain::discover()
    };
    let rebuilt = cook(&manifest, &output, &tools)?;
    println!("Cooked {rebuilt} pipelines (unchanged artifacts reused)");
    Ok(())
}
