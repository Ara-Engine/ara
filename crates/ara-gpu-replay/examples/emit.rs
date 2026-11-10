//! Explicit opt-in writer for inspecting generated sources and validating SPIR-V.
use anyhow::Result;
use ara_gpu_replay::{Pass, msl, spirv};
fn main() -> Result<()> {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/wgsl-replay");
    std::fs::create_dir_all(&root)?;
    for (name, pass) in [
        ("downsample", Pass::Downsample),
        ("upsample", Pass::Upsample),
        ("tone", Pass::Tone),
    ] {
        std::fs::write(root.join(format!("{name}.metal")), msl(pass)?.source)?;
        for stage in spirv(pass)? {
            let bytes: Vec<u8> = stage.words.iter().flat_map(|w| w.to_le_bytes()).collect();
            std::fs::write(root.join(format!("{name}-{}.spv", stage.entry)), bytes)?;
        }
    }
    println!("{}", root.display());
    Ok(())
}
