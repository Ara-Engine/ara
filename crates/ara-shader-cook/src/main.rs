use anyhow::{Context, Result, ensure};
use ara_shader_cook::{Toolchain, cook_with_variant, source_fingerprint};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut raw: Vec<_> = std::env::args_os().skip(1).collect();
    let watch = raw.iter().any(|arg| arg == "--watch");
    let promote = raw.iter().any(|arg| arg == "--promote");
    raw.retain(|arg| arg != "--watch" && arg != "--promote");
    let variant = if let Some(index) = raw.iter().position(|arg| arg == "--variant") {
        raw.remove(index);
        ensure!(index < raw.len(), "--variant needs a recipe path");
        Some(PathBuf::from(raw.remove(index)))
    } else {
        None
    };
    ensure!(
        !promote || (!watch && variant.is_some()),
        "--promote requires --variant and cannot watch"
    );
    let mut args = raw.into_iter();
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
    ensure!(args.next().is_none(), "unexpected shader cook argument");
    let mut previous = String::new();
    loop {
        let recipe = variant.as_ref().map(std::fs::read).transpose()?;
        let permutation = recipe
            .as_deref()
            .map(serde_json::from_slice::<ara_shader_cook::variant::Variant>)
            .transpose()?;
        let current = format!("{}{:?}", source_fingerprint(&manifest)?, recipe);
        if current != previous {
            previous = current;
            let started = std::time::Instant::now();
            match cook_with_variant(&manifest, &output, &tools, permutation.as_ref()) {
                Ok(rebuilt) => {
                    if promote {
                        ara_shader_cook::variant::promote(
                            &manifest,
                            permutation.as_ref().context("promotion recipe")?,
                        )?;
                    }
                    println!(
                        "Cooked {rebuilt} pipelines in {:.3}s (unchanged artifacts reused)",
                        started.elapsed().as_secs_f64()
                    )
                }
                Err(error) if watch => {
                    eprintln!("Cook failed; last good cohort retained: {error:#}")
                }
                Err(error) => return Err(error),
            }
        }
        if !watch {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
