//! Full generated material evaluation on analytic sphere/plane geometry, off the UI thread.
use ara_gpu::{
    Access, Barrier, Queue, Resource, RootConstants,
    commands::{Attachment, Command},
    vk,
    vulkan::VulkanDevice,
};
use ara_render::capture::{Capture, CaptureRequest, FeatureCapture, ShaderMetadata, ViewMode};
use std::{collections::BTreeMap, path::Path};
pub(super) fn capture(
    root: &Path,
    material: &ara_material::Material,
    plane: bool,
) -> anyhow::Result<image::RgbaImage> {
    let generated = ara_material::generate(material, &Default::default())
        .map_err(|d| anyhow::anyhow!("{d:?}"))?;
    let body = generated
        .source
        .split("// Offline verification entry.")
        .next()
        .ok_or_else(|| anyhow::anyhow!("material preview source"))?;
    let source=body.replace("textures[16]","textures[128]").replace("RWStructuredBuffer<float4> buffers[4]","RWByteAddressBuffer buffers[64]").replace("SamplerState linearSampler : register(s0, space0);","SamplerState samplers[3] : register(s0, space0);")
        .replace("return buffers[NonUniformResourceIndex(draw.bufferIndex)][draw.materialIndex + offset];","return buffers[NonUniformResourceIndex(draw.bufferIndex)].Load<float4>((draw.materialIndex+offset)*16);")
        .replace("linearSampler","samplers[0]");
    let source = format!(
        "#define PREVIEW_SURFACE {}\n{source}\n{}",
        u32::from(matches!(
            material.domain,
            ara_material::Domain::Surface | ara_material::Domain::Decal
        )),
        include_str!("material_preview.slang")
    );
    let hash = ara_asset::Digest::of(source.as_bytes()).to_string();
    let base = root.join(".ara/cache/material-previews");
    let folder = base.join("source").join(&hash);
    let cooked = base.join(&hash);
    if !cooked.join("preview.json").exists() {
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join("preview.slang"), source)?;
        let manifest = serde_json::json!({"preview":{"source":"preview.slang","entries":{"previewVertex":"vertex","previewFragment":"fragment"},"state":ara_gpu::cooked::RenderState{color_formats:vec![37,100],..Default::default()}}});
        let path = folder.join("pipelines.json");
        std::fs::write(&path, ara_json::to_canonical_string(&manifest))?;

        ara_shader_cook::cook(&path, &cooked, &ara_shader_cook::Toolchain::discover())?;
    }
    let pipeline: ara_gpu::cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("preview.json"))?)?;
    let mut gpu = VulkanDevice::new(&[], base.join("gpu"))?;
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
            384,
            384,
            format,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let read = gpu.create_buffer(384 * 384 * 4, vk::BufferUsageFlags::TRANSFER_DST, true)?;
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
        size: [384, 384],
    });
    recorder.push(Command::Pipeline(program));
    recorder.push(Command::Constants(RootConstants {
        words: [0, 0, index, u32::from(plane), 0, 1f32.to_bits(), 0, 0],
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
        size: [384, 384],
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
            device: &gpu.context().info,
            timings: &timings,
            shaders: &metadata,
        },
        &request,
    )?;
    gpu.context().assert_clean()?;
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
    gpu: &mut VulkanDevice,
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
