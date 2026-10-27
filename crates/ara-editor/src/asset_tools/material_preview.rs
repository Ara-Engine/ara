//! Full generated material evaluation on analytic sphere/plane geometry, off the UI thread.
//! A [`Shot`] picks the geometry and whether values show lit and tone mapped (thumbnails)
//! or raw (in-node previews of intermediate values).
use super::studio::MASTER;
use ara_gpu::{
    Access, Barrier, GpuDevice, Queue, Resource, RootConstants,
    commands::{Attachment, Command},
    vk,
};
use ara_render::capture::{Capture, CaptureRequest, FeatureCapture, ShaderMetadata, ViewMode};
use std::{collections::BTreeMap, path::Path};
/// How a material is photographed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Shot {
    /// Geometry under the camera.
    pub shape: Shape,
    /// Show the raw value (sRGB-encoded, no tone curve): an intermediate node's output.
    pub data: bool,
}
/// Preview geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Shape {
    /// The studio sphere (thumbnails, world-space values).
    Sphere,
    /// A quad at the studio bearing.
    Plane,
    /// The UV square filling the image head-on (2D values such as UVs and noise).
    Swatch,
}
impl Shot {
    /// A lit thumbnail on the sphere or the plane.
    pub fn surface(plane: bool) -> Self {
        Self {
            shape: if plane { Shape::Plane } else { Shape::Sphere },
            data: false,
        }
    }
    /// Root-constant flags read by `material_preview.slang`.
    fn flags(self) -> u32 {
        u32::from(self.shape == Shape::Plane)
            | u32::from(self.data) << 1
            | u32::from(self.shape == Shape::Swatch) << 2
    }
    /// Stable cache identity.
    pub fn tag(self) -> u8 {
        self.flags() as u8
    }
}

/// Generates and cooks a material's preview shader into the project cache, once per
/// generated source (the cooked folder is named by its hash); returns that folder.
///
/// Previews only run on Vulkan, so this is a SPIR-V-only cook with both stages
/// compiled side by side (one compiler process each) instead of the engine cook's
/// sequential SPIR-V and DXIL compiles: the compiler is most of a cold preview's
/// latency. Cooks publish by renaming a private staging folder, so concurrent cooks of
/// one source (the page preview and an in-node preview) never see half a folder.
pub(super) fn cook(
    root: &Path,
    material: &ara_material::Material,
) -> anyhow::Result<std::path::PathBuf> {
    let generated = ara_material::generate(material, &Default::default())
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    let body = generated
        .source
        .split("// Offline verification entry.")
        .next()
        .ok_or_else(|| anyhow::anyhow!("material preview source"))?;
    let source=body.replace("textures[16]","textures[128]").replace("RWStructuredBuffer<float4> buffers[4]","RWByteAddressBuffer buffers[]").replace("SamplerState linearSampler : register(s0, space0);","SamplerState samplers[3] : register(s0, space0);")
        .replace("return buffers[NonUniformResourceIndex(draw.bufferIndex)][draw.materialIndex + offset];","return buffers[NonUniformResourceIndex(draw.bufferIndex)].Load<float4>((draw.materialIndex+offset)*16);")
        .replace("linearSampler","samplers[0]");
    let source = format!(
        "#define PREVIEW_SURFACE {}
{source}
{}",
        u32::from(matches!(
            material.domain,
            ara_material::Domain::Surface | ara_material::Domain::Decal
        )),
        include_str!("material_preview.slang")
    );
    let hash = ara_asset::Digest::of(source.as_bytes()).to_string();
    let base = root.join(".ara/cache/material-previews");
    let cooked = base.join(&hash);
    if cooked.join("preview.json").exists() {
        return Ok(cooked);
    }
    static STAGING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let staging = base.join("staging").join(format!(
        "{hash}-{}-{}",
        std::process::id(),
        STAGING.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = compile(&staging, &source, &hash).and_then(|()| {
        match std::fs::rename(&staging, &cooked) {
            Ok(()) => Ok(()),
            // Another cook of the same source published first.
            Err(_) if cooked.join("preview.json").exists() => Ok(()),
            Err(error) => Err(error.into()),
        }
    });
    let _ = std::fs::remove_dir_all(&staging);
    result.map(|()| cooked)
}

/// Compiles the preview's two stages into `folder` side by side, validates them and
/// writes `preview.json` there.
fn compile(folder: &Path, source: &str, hash: &str) -> anyhow::Result<()> {
    use ara_gpu::cooked::{CookedPipeline, CookedStage, RenderState, Stage};
    use sha2::Digest as _;
    use std::process::{Command, Stdio};
    let tools = ara_shader_cook::Toolchain::discover();
    let path = folder.join("preview.slang");
    std::fs::write(&path, source)?;
    let stages = [
        ("previewVertex", Stage::Vertex),
        ("previewFragment", Stage::Fragment),
    ];
    let spawn = |command: &mut Command| {
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("starting {command:?}: {e}"))
    };
    let finish = |child: std::process::Child| -> anyhow::Result<String> {
        let output = child.wait_with_output()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        anyhow::ensure!(output.status.success(), "{text}");
        Ok(text)
    };
    let compiles = stages
        .iter()
        .map(|&(entry, stage)| {
            let vertex = stage == Stage::Vertex;
            let mut command = Command::new(&tools.slangc);
            command.arg(&path).args([
                "-entry",
                entry,
                "-stage",
                if vertex { "vertex" } else { "fragment" },
                "-target",
                "spirv",
                "-profile",
                "sm_6_6+spirv_1_6",
                "-O2",
                "-matrix-layout-column-major",
                "-D",
                "VULKAN=1",
            ]);
            if vertex {
                command.arg("-fvk-invert-y");
            }
            command.arg("-o").arg(folder.join(format!("{entry}.spv")));
            spawn(&mut command)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    // The compiler's version (cook provenance) is asked once per process, alongside.
    static COMPILER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let version = match COMPILER.get() {
        Some(_) => None,
        None => Some(spawn(Command::new(&tools.slangc).arg("-version"))?),
    };
    for child in compiles {
        finish(child)?;
    }
    if let Some(version) = version {
        let _ = COMPILER.set(finish(version)?.trim().to_owned());
    }
    let compiler = COMPILER.get().cloned().unwrap_or_default();
    let validations = stages
        .iter()
        .map(|(entry, _)| {
            spawn(
                Command::new(&tools.spirv_val)
                    .args(["--target-env", "vulkan1.3"])
                    .arg(folder.join(format!("{entry}.spv"))),
            )
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    for child in validations {
        finish(child)?;
    }
    let stages = stages
        .iter()
        .map(|&(entry, stage)| -> anyhow::Result<CookedStage> {
            let spirv = format!("{entry}.spv");
            Ok(CookedStage {
                source_entry: entry.into(),
                spirv_entry: "main".into(),
                stage,
                spirv_hash: format!(
                    "{:x}",
                    sha2::Sha256::digest(std::fs::read(folder.join(&spirv))?)
                ),
                spirv,
                // Previews never leave Vulkan.
                dxil: String::new(),
                dxil_hash: String::new(),
                metal: None,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    ara_gpu::cooked::write_json(
        &folder.join("preview.json"),
        &CookedPipeline {
            version: 1,
            key: hash.into(),
            binding_layout_hash: ara_gpu::cooked::binding_layout_hash(),
            compiler,
            stages,
            state: RenderState {
                color_formats: vec![37, 100],
                ..Default::default()
            },
        },
    )
}

pub(super) fn capture(
    root: &Path,
    material: &ara_material::Material,
    shot: Shot,
) -> anyhow::Result<image::RgbaImage> {
    let cooked = cook(root, material)?;
    // The cooked folder is named by the generated source's hash.
    let hash = cooked
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    let base = root.join(".ara/cache/material-previews");
    let pipeline: ara_gpu::cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("preview.json"))?)?;
    let mut gpu = GpuDevice::new(&[], base.join("gpu"))?;
    let program = gpu.create_pipeline(&pipeline, &cooked)?;
    let mut recorder = gpu.begin(Queue::Graphics);
    let mut textures = BTreeMap::new();
    let packed = ara_material::Instance::default()
        .pack(material, |source, _ty| {
            let key = ara_json::to_canonical_string(
                &ara_json::to_value(source).map_err(|e| crate::invalid(e.to_string()))?,
            );
            if let Some(index) = textures.get(&key) {
                return Ok(*index);
            }
            let image = texture(root, source, &mut gpu, &mut recorder)
                .map_err(|e| crate::invalid(e.to_string()))?;
            let index = gpu
                .sampled_image(image, false)
                .map_err(|e| crate::invalid(e.to_string()))?
                .index;
            textures.insert(key, index);
            Ok(index)
        })
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    let rows = if packed.rows.is_empty() {
        vec![[0.; 4]]
    } else {
        packed.rows
    };
    let bytes = rows
        .iter()
        .flat_map(|r| r.iter().flat_map(|v| v.to_le_bytes()))
        .collect::<Vec<_>>();
    let uniform = gpu.create_buffer(
        bytes.len() as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER,
        true,
    )?;
    gpu.upload(uniform, &bytes)?;
    let index = gpu.storage_buffer(uniform)?.index;
    barrier(
        &mut recorder,
        Resource::Buffer(uniform),
        Access::HostWrite,
        Access::ShaderRead,
    );
    let mut outputs = Vec::new();
    let mut reads = Vec::new();
    for format in [vk::Format::R8G8B8A8_UNORM, vk::Format::R32_SFLOAT] {
        let image = gpu.create_image(
            MASTER,
            MASTER,
            format,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let read = gpu.create_buffer(
            u64::from(MASTER * MASTER * 4),
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        barrier(
            &mut recorder,
            Resource::Image(image),
            Access::Undefined,
            Access::Color,
        );
        outputs.push(image);
        reads.push(read);
    }
    recorder.push(Command::BeginPass("Editor.MaterialPreview".into()));
    recorder.push(Command::BeginRendering {
        colors: outputs
            .iter()
            .map(|image| Attachment {
                image: *image,
                clear: Some([0.; 4]),
            })
            .collect(),
        depth: None,
        size: [MASTER, MASTER],
    });
    recorder.push(Command::Pipeline(program));
    recorder.push(Command::Constants(RootConstants {
        words: [0, 0, index, shot.flags(), 0, 1f32.to_bits(), 0, 0],
    }));
    recorder.push(Command::Fullscreen);
    recorder.push(Command::EndRendering);
    recorder.push(Command::EndPass);
    for (image, read) in outputs.into_iter().zip(&reads) {
        barrier(
            &mut recorder,
            Resource::Image(image),
            Access::Color,
            Access::CopySource,
        );
        barrier(
            &mut recorder,
            Resource::Buffer(*read),
            Access::Undefined,
            Access::CopyDestination,
        );
        recorder.push(Command::ReadImage {
            source: image,
            destination: *read,
        });
        barrier(
            &mut recorder,
            Resource::Buffer(*read),
            Access::CopyDestination,
            Access::HostRead,
        );
    }
    let submission = gpu.submit(recorder.finish(), &[])?;
    let timings = gpu.resolve(submission)?;
    let rgba = gpu.read(reads[0])?;
    let ids = gpu
        .read(reads[1])?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v) as u32)
        .collect();
    let request = CaptureRequest {
        camera: ara_render::scene::CameraSpec::LookAt {
            position: glam::Vec3::new(2.8, 1.7, 3.8),
            target: glam::Vec3::ZERO,
            vertical_fov: 0.62,
        },
        size: [MASTER, MASTER],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 1,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let metadata = ShaderMetadata {
        content_key: pipeline.key,
        compiler: pipeline.compiler,
        binding_layout_hash: pipeline.binding_layout_hash,
    };
    let names = [(1, material.name.clone())];
    let capture = Capture::from_feature(
        FeatureCapture {
            rgba,
            ids,
            names: &names,
            hash: &hash,
            device: gpu.info(),
            timings: &timings,
            shaders: &metadata,
        },
        &request,
    )?;
    gpu.assert_clean()?;
    Ok(capture.image)
}
fn barrier(
    recorder: &mut ara_gpu::commands::Recorder,
    resource: Resource,
    before: Access,
    after: Access,
) {
    recorder.push(Command::Barrier(Barrier {
        resource,
        before,
        after,
        handoff: None,
    }));
}
fn texture(
    root: &Path,
    source: &ara_material::TextureSource,
    gpu: &mut GpuDevice,
    recorder: &mut ara_gpu::commands::Recorder,
) -> anyhow::Result<ara_gpu::Image> {
    let image = match source {
        ara_material::TextureSource::Default(value) => image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba(match value {
                ara_material::TextureDefault::White => [255; 4],
                ara_material::TextureDefault::Black => [0, 0, 0, 255],
                ara_material::TextureDefault::FlatNormal => [128, 128, 255, 255],
            }),
        ),
        ara_material::TextureSource::Asset(reference) => {
            let reference =
                ara_schema::grammar::parse_ref(&reference.0).map_err(anyhow::Error::msg)?;
            let path = root
                .join(
                    reference
                        .path
                        .ok_or_else(|| anyhow::anyhow!("texture reference needs a path"))?,
                )
                .canonicalize()?;
            anyhow::ensure!(
                path.starts_with(root.canonicalize()?),
                "texture escapes project"
            );
            image::open(path)?.to_rgba8()
        }
    };
    let texture = gpu.create_image(
        image.width(),
        image.height(),
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
    )?;
    let buffer = gpu.create_buffer(
        image.as_raw().len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        true,
    )?;
    gpu.upload(buffer, image.as_raw())?;
    barrier(
        recorder,
        Resource::Buffer(buffer),
        Access::HostWrite,
        Access::CopySource,
    );
    barrier(
        recorder,
        Resource::Image(texture),
        Access::Undefined,
        Access::CopyDestination,
    );
    recorder.push(Command::UploadImage {
        source: buffer,
        destination: texture,
    });
    barrier(
        recorder,
        Resource::Image(texture),
        Access::CopyDestination,
        Access::ShaderRead,
    );
    Ok(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs slangc (`VULKAN_SDK`). The page preview and an in-node preview may cook
    /// one source at the same time: both get the one published folder, whole.
    #[test]
    fn concurrent_cooks_of_one_source_publish_one_valid_folder() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-editor/preview-cook")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&root);
        let material = ara_material::dsl::parse(
            "material Tint : surface budget 1 { param tint : ColorLinear = linear(0.8, 0.4, 0.1); output baseColor = tint; }",
            "gr_tint",
        )
        .expect("material");
        let folders: Vec<std::path::PathBuf> = std::thread::scope(|scope| {
            let cooks: Vec<_> = (0..2)
                .map(|_| scope.spawn(|| cook(&root, &material).expect("cook")))
                .collect();
            cooks
                .into_iter()
                .map(|c| c.join().expect("cook thread"))
                .collect()
        });
        assert_eq!(folders[0], folders[1]);
        let pipeline: ara_gpu::cooked::CookedPipeline = serde_json::from_slice(
            &std::fs::read(folders[0].join("preview.json")).expect("published"),
        )
        .expect("pipeline");
        assert_eq!(pipeline.stages.len(), 2);
        for stage in &pipeline.stages {
            use sha2::Digest as _;
            let bytes = std::fs::read(folders[0].join(&stage.spirv)).expect("SPIR-V");
            assert_eq!(
                format!("{:x}", sha2::Sha256::digest(&bytes)),
                stage.spirv_hash
            );
            assert_eq!(stage.spirv_entry, "main");
        }
        let staging = root.join(".ara/cache/material-previews/staging");
        assert_eq!(
            std::fs::read_dir(&staging).map_or(0, |d| d.count()),
            0,
            "no staging folder is left behind"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
