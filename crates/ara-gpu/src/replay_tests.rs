//! Shared GPU replay runner: Metal uses generated MSL; Vulkan loads generated SPIR-V.
use crate::{
    commands::{Attachment, Command},
    cooked::{CookedPipeline, CookedStage, RenderState, Stage},
    descriptors::{Format, ImageDesc},
    texture::{ImageShape, TextureDesc, TextureDimension},
    *,
};
use anyhow::Result;
use ara_gpu_replay::{Pass, f16};
/// A fresh output root prevents parallel tests/processes from racing artifact writes.
pub(crate) fn artifact_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-gpu/wgsl-replay")
        .join(format!(
            "{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
}
fn barrier(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
fn texture(
    gpu: &mut GpuDevice,
    size: [u32; 2],
    format: Format,
    storage: bool,
    attachment: bool,
) -> Result<Image> {
    Ok(gpu.create_texture(&TextureDesc {
        image: ImageDesc {
            size,
            format,
            sampled: !attachment,
            storage,
            attachment,
            copy_source: true,
            copy_destination: true,
        },
        shape: ImageShape::default(),
    })?)
}
fn half_bytes(pixels: &[[f32; 4]]) -> Vec<u8> {
    pixels
        .iter()
        .flatten()
        .flat_map(|v| f16::from_f32(*v).to_bits().to_le_bytes())
        .collect()
}
fn upload(
    gpu: &mut GpuDevice,
    rec: &mut commands::Recorder,
    image: Image,
    bytes: &[u8],
) -> Result<()> {
    let buffer = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(buffer, bytes)?;
    rec.barriers([
        barrier(
            Resource::Buffer(buffer),
            Access::HostWrite,
            Access::CopySource,
        ),
        barrier(
            Resource::Image(image),
            Access::Undefined,
            Access::CopyDestination,
        ),
    ]);
    rec.push(Command::UploadImage {
        source: buffer,
        destination: image,
    });
    rec.barriers([barrier(
        Resource::Image(image),
        Access::CopyDestination,
        Access::ShaderRead,
    )]);
    Ok(())
}
/// Execute both down levels, the reverse up level and raster tone; compare every output.
pub(crate) fn run(
    gpu: &mut GpuDevice,
    pipelines: [Pipeline; 3],
) -> Result<Vec<commands::PassTiming>> {
    let fixture = ara_gpu_replay::fixture();
    let cpu_down0 = ara_gpu_replay::downsample(&fixture.hdr, [9, 6]);
    let cpu_down1 = ara_gpu_replay::downsample(&cpu_down0, [5, 3]);
    let cpu_up = ara_gpu_replay::upsample(&cpu_down0, &cpu_down1);
    let cpu_tone = ara_gpu_replay::tone(&fixture.hdr, &cpu_up, &fixture);
    let hdr = texture(gpu, fixture.hdr.size, Format::Rgba16Float, false, false)?;
    let down0 = texture(gpu, [9, 6], Format::Rgba16Float, true, false)?;
    let down1 = texture(gpu, [5, 3], Format::Rgba16Float, true, false)?;
    let up = texture(gpu, [9, 6], Format::Rgba16Float, true, false)?;
    let tone = texture(gpu, fixture.hdr.size, Format::Rgba32Float, false, true)?;
    let lut = gpu.create_texture(&TextureDesc {
        image: ImageDesc {
            size: [fixture.lut_size; 2],
            format: Format::Rgba16Float,
            sampled: true,
            storage: false,
            attachment: false,
            copy_source: false,
            copy_destination: true,
        },
        shape: ImageShape {
            dimension: TextureDimension::D3,
            depth: fixture.lut_size,
            ..Default::default()
        },
    })?;
    let settings_buffer = gpu.create_buffer(80, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    gpu.upload(settings_buffer, bytemuck::cast_slice(&fixture.settings))?;
    let settings = gpu.storage_buffer(settings_buffer)?;
    let sampler = gpu.sampler(&sampler::SamplerDesc::default())?;
    let samples = [hdr, down0, down1, up, lut]
        .map(|i| gpu.sampled_image(i, false))
        .into_iter()
        .collect::<crate::Result<Vec<_>>>()?;
    let outputs = [down0, down1, up]
        .map(|i| gpu.storage_image(i))
        .into_iter()
        .collect::<crate::Result<Vec<_>>>()?;
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barriers([barrier(
        Resource::Buffer(settings_buffer),
        Access::HostWrite,
        Access::ShaderRead,
    )]);
    upload(gpu, &mut rec, hdr, &half_bytes(&fixture.hdr.pixels))?;
    upload(gpu, &mut rec, lut, &half_bytes(&fixture.lut))?;
    for (i, (name, input, second, out, size)) in [
        ("Downsample0", 0, 0, 0, [9u32, 6]),
        ("Downsample1", 1, 0, 1, [5, 3]),
        ("Upsample", 1, 2, 2, [9, 6]),
    ]
    .into_iter()
    .enumerate()
    {
        rec.barriers([barrier(
            Resource::Image([down0, down1, up][i]),
            Access::Undefined,
            Access::Storage,
        )]);
        rec.push(Command::BeginPass(name.into()));
        rec.push(Command::Pipeline(pipelines[usize::from(i == 2)]));
        rec.push(Command::Constants(RootConstants {
            words: [
                samples[input].index,
                samples[second].index,
                samples[0].index,
                samples[0].index,
                outputs[out].index,
                settings.index,
                sampler.index,
                samples[4].index,
            ],
        }));
        rec.push(Command::Dispatch([
            size[0].div_ceil(8),
            size[1].div_ceil(8),
            1,
        ]));
        rec.push(Command::EndPass);
        rec.barriers([barrier(
            Resource::Image([down0, down1, up][i]),
            Access::Storage,
            Access::ShaderRead,
        )]);
    }
    rec.barriers([barrier(
        Resource::Image(tone),
        Access::Undefined,
        Access::Color,
    )]);
    rec.push(Command::BeginPass("Tone".into()));
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: tone,
            clear: Some([0.; 4]),
        }],
        depth: None,
        size: fixture.hdr.size,
    });
    rec.push(Command::Pipeline(pipelines[2]));
    rec.push(Command::Constants(RootConstants {
        words: [
            samples[0].index,
            samples[3].index,
            0,
            0,
            0,
            settings.index,
            sampler.index,
            samples[4].index,
        ],
    }));
    rec.push(Command::Fullscreen);
    rec.push(Command::EndRendering);
    rec.push(Command::EndPass);
    let references = [cpu_down0, cpu_down1, cpu_up, cpu_tone];
    let mut readbacks = Vec::new();
    for (i, image) in [down0, down1, up, tone].into_iter().enumerate() {
        let bpp = if i == 3 { 16 } else { 8 };
        let size = references[i].pixels.len() * bpp;
        let out = gpu.create_readback_buffer(size as u64)?;
        rec.barriers([barrier(
            Resource::Image(image),
            if i == 3 {
                Access::Color
            } else {
                Access::ShaderRead
            },
            Access::CopySource,
        )]);
        rec.push(Command::ReadImage {
            source: image,
            destination: out,
        });
        rec.barriers([barrier(
            Resource::Buffer(out),
            Access::CopyDestination,
            Access::HostRead,
        )]);
        readbacks.push(out);
    }
    let point = gpu.submit(rec.finish(), &[])?;
    gpu.wait(point)?;
    #[derive(serde::Serialize)]
    struct Comparison {
        pass: &'static str,
        extent: [u32; 2],
        maximum_absolute_error: f32,
    }
    let mut comparisons = Vec::new();
    for (i, (output, reference)) in readbacks.into_iter().zip(references).enumerate() {
        let data = gpu.read(output)?;
        let actual: Vec<f32> = if i == 3 {
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
                .collect()
        } else {
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|v| f16::from_bits(u16::from_le_bytes(*v)).to_f32())
                .collect()
        };
        let maximum = actual
            .iter()
            .zip(reference.pixels.iter().flatten())
            .map(|(a, b)| {
                assert!(a.is_finite());
                (a - b).abs()
            })
            .fold(0f32, f32::max);
        let name = ["Downsample0", "Downsample1", "Upsample", "Tone"][i];
        eprintln!(
            "WGSL replay {name} {:?}: max_absolute_error={maximum}",
            reference.size
        );
        assert!(
            maximum <= 0.003,
            "{name} differs from CPU reference: {maximum}"
        );
        comparisons.push(Comparison {
            pass: name,
            extent: reference.size,
            maximum_absolute_error: maximum,
        });
    }
    let path = artifact_dir("comparison");
    std::fs::create_dir_all(&path)?;
    cooked::write_json(&path.join("comparison.json"), &comparisons)?;
    Ok(gpu.resolve(point)?)
}
pub(crate) fn state(pass: Pass) -> RenderState {
    RenderState {
        color_formats: if matches!(pass, Pass::Tone) {
            vec![vk::Format::R32G32B32A32_SFLOAT.as_raw()]
        } else {
            vec![]
        },
        ..Default::default()
    }
}
/// Generate genuine bindless SPIR-V artifacts on either platform, for the PC test.
#[test]
fn emit_naga_spirv_for_portable_replay() -> Result<()> {
    let path = artifact_dir("spirv");
    for pass in [Pass::Downsample, Pass::Upsample, Pass::Tone] {
        emit(pass, &path)?;
    }
    eprintln!("WGSL SPIR-V artifacts: {}", path.display());
    Ok(())
}
fn emit(pass: Pass, path: &std::path::Path) -> Result<CookedPipeline> {
    use sha2::{Digest, Sha256};
    std::fs::create_dir_all(path)?;
    let mut stages = Vec::new();
    for stage in ara_gpu_replay::spirv(pass)? {
        let name = format!("{pass:?}-{}.spv", stage.entry);
        let bytes: Vec<u8> = stage.words.iter().flat_map(|w| w.to_le_bytes()).collect();
        std::fs::write(path.join(&name), &bytes)?;
        stages.push(CookedStage {
            source_entry: stage.entry.clone(),
            spirv_entry: stage.entry,
            stage: match stage.stage {
                ara_gpu_replay::ShaderStage::Compute => Stage::Compute,
                ara_gpu_replay::ShaderStage::Vertex => Stage::Vertex,
                ara_gpu_replay::ShaderStage::Fragment => Stage::Fragment,
                _ => anyhow::bail!("unsupported replay stage"),
            },
            spirv: name,
            dxil: String::new(),
            spirv_hash: format!("{:x}", Sha256::digest(&bytes)),
            dxil_hash: String::new(),
            metal: None,
        });
    }
    let pipeline = CookedPipeline {
        version: 1,
        key: format!("naga-replay-{pass:?}"),
        binding_layout_hash: cooked::binding_layout_hash(),
        compiler: "naga 30.0.1 WGSL replay".into(),
        stages,
        state: state(pass),
    };
    cooked::write_json(&path.join(format!("{pass:?}.json")), &pipeline)?;
    Ok(pipeline)
}
#[test]
#[cfg(not(target_os = "macos"))]
fn naga_spirv_replays_renderlab_post_against_cpu() -> Result<()> {
    let path = artifact_dir("vulkan");
    let mut gpu = GpuDevice::new_raster(&[], path.join("cache"))?;
    let mut pipelines = Vec::new();
    for pass in [Pass::Downsample, Pass::Upsample, Pass::Tone] {
        let meta = emit(pass, &path)?;
        pipelines.push(gpu.create_pipeline(&meta, &path)?);
    }
    run(
        &mut gpu,
        pipelines
            .try_into()
            .map_err(|_| anyhow::anyhow!("three pipelines"))?,
    )?;
    gpu.assert_clean()?;
    Ok(())
}
