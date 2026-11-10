#![warn(missing_docs)]
//! Test-only replay of Renderlab WGSL through naga, with independent CPU references.
mod cpu;
mod metal;
mod spirv;

pub use cpu::{Fixture, Image, downsample, fixture, tone, upsample};
pub use half::f16;
pub use metal::msl;
pub use naga::ShaderStage;
pub use spirv::spirv;

/// The unchanged Renderlab filter source, including the selected bloom kernels.
pub const POST: &str = include_str!("../shaders/post.wgsl");
/// The unchanged Renderlab fullscreen vertex and tone fragment source.
pub const TONE: &str = include_str!("../shaders/post_tone.wgsl");
/// One pass family in the replay.
#[derive(Debug, Clone, Copy)]
pub enum Pass {
    /// Nine-tap tent downsample into RGBA16Float.
    Downsample,
    /// Convex 0.65 mix with a tent upsample into RGBA16Float.
    Upsample,
    /// Fullscreen tone, grading and a sampled 3D LUT.
    Tone,
}
impl Pass {
    fn source(self) -> &'static str {
        if matches!(self, Self::Tone) {
            TONE
        } else {
            POST
        }
    }
    fn entry(self) -> &'static str {
        match self {
            Self::Downsample => "downsample",
            Self::Upsample => "upsample",
            Self::Tone => "fs",
        }
    }
}
/// Naga-generated MSL with a checked shim that reads Ara's heap from buffer 2.
pub struct MetalProgram {
    /// Complete source except the caller's TopLevel ABI declaration.
    pub source: String,
    /// Selected compute or fragment entry point.
    pub entry: String,
    /// Fullscreen vertex entry, present only for tone.
    pub vertex_entry: Option<String>,
}
/// One SPIR-V entry compiled against Ara's global descriptor bindings and roots.
pub struct SpirvStage {
    /// Native shader stage.
    pub stage: ShaderStage,
    /// Naga's entry point name.
    pub entry: String,
    /// Little-endian SPIR-V words.
    pub words: Vec<u32>,
}
fn parse(source: &str) -> anyhow::Result<naga::Module> {
    naga::front::wgsl::parse_str(source).map_err(|e| anyhow::anyhow!(e.emit_to_string(source)))
}
fn validate(module: &naga::Module) -> anyhow::Result<naga::valid::ModuleInfo> {
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(module)
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_frontends_compile_all_three_passes() {
        for pass in [Pass::Downsample, Pass::Upsample, Pass::Tone] {
            let metal = msl(pass).expect("original WGSL translates to MSL heap shim");
            assert!(
                metal
                    .source
                    .contains("constant TopLevel& top [[buffer(2)]]")
            );
            let stages = spirv(pass).expect("heap WGSL translates to SPIR-V");
            assert_eq!(stages.len(), if matches!(pass, Pass::Tone) { 2 } else { 1 });
            for stage in stages {
                assert_eq!(stage.words[0], 0x07230203);
            }
        }
    }
    #[test]
    fn scalar_bloom_preserves_constant_energy_at_odd_extents() {
        let image = Image {
            size: [17, 11],
            pixels: vec![[1.; 4]; 17 * 11],
        };
        let down = downsample(&image, [9, 6]);
        let coarse = downsample(&down, [5, 3]);
        let up = upsample(&down, &coarse);
        assert!(up.pixels.iter().all(|p| *p == [1.; 4]));
        let fixture = fixture();
        let output = tone(&fixture.hdr, &up, &fixture);
        assert_eq!(output.pixels.len(), 17 * 11);
        assert!(
            output
                .pixels
                .iter()
                .flatten()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        );
    }
}
