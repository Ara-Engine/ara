//! Explicit CPU-only evaluator turnaround/parity probe; never writes measurements.
use anyhow::{Context, Result};
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let reference = image::open(args.next().context("reference PNG required")?)?.into_rgba8();
    let actual = image::open(args.next().context("actual PNG required")?)?.into_rgba8();
    let began = std::time::Instant::now();
    let result = ara_pathtrace::flip::flip(&reference, &actual, 67.)?;
    println!(
        "{}",
        serde_json::json!({"mean":result.mean,"seconds":began.elapsed().as_secs_f64(),"size":reference.dimensions(),"ppd":67})
    );
    Ok(())
}
