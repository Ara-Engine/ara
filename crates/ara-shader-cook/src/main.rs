use anyhow::{Context, Result};
use ara_shader_cook::{Toolchain, cook, source_fingerprint};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut raw: Vec<_> = std::env::args_os().skip(1).collect();
    let watch = raw.iter().any(|arg| arg == "--watch");
    raw.retain(|arg| arg != "--watch");
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
    let mut previous = String::new();
    loop {
        let current = source_fingerprint(&manifest)?;
        if current != previous {
            previous = current;
            let started = std::time::Instant::now();
            match cook(&manifest, &output, &tools) {
                Ok(rebuilt) => println!(
                    "Cooked {rebuilt} pipelines in {:.3}s (unchanged artifacts reused)",
                    started.elapsed().as_secs_f64()
                ),
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
