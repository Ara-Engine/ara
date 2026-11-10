//! Shader-visible image subviews, sampler addressing and independent MRT blending.
use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, GpuDevice, Pipeline, Queue, Resource, RootConstants,
    commands::{Attachment, Command},
    cooked::CookedPipeline,
    descriptors::{Format, ImageDesc},
    raster::*,
    sampler::{AddressMode, Filter, SamplerDesc},
    texture::*,
    vk,
};
fn device(name: &str) -> Result<GpuDevice> {
    Ok(GpuDevice::new_raster(
        &[],
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-gpu/features")
            .join(name),
    )?)
}
fn edge(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
fn pipeline(gpu: &mut GpuDevice, name: &str) -> Result<Pipeline> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/feature_cooked");
    let metadata: CookedPipeline =
        serde_json::from_slice(&std::fs::read(dir.join(format!("{name}.json")))?)?;
    Ok(gpu.create_pipeline(&metadata, &dir)?)
}
#[test]
fn sampler_addressing_filtering_and_anisotropy_change_device_reads() -> Result<()> {
    let mut gpu = device("samplers")?;
    let image = gpu.create_image(
        2,
        1,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
    )?;
    let input = gpu.create_buffer(8, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(input, &[255, 0, 0, 255, 0, 0, 255, 255])?;
    let output = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    let image_slot = gpu.sampled_image(image, false)?.index;
    let output_slot = gpu.storage_buffer(output)?.index;
    let p = pipeline(&mut gpu, "samplers")?;
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barriers([
        edge(
            Resource::Buffer(input),
            Access::HostWrite,
            Access::CopySource,
        ),
        edge(
            Resource::Image(image),
            Access::Undefined,
            Access::CopyDestination,
        ),
    ]);
    rec.push(Command::UploadImage {
        source: input,
        destination: image,
    });
    rec.barriers([edge(
        Resource::Image(image),
        Access::CopyDestination,
        Access::ShaderRead,
    )]);
    let point = gpu.submit(rec.finish(), &[])?;
    gpu.wait(point)?;
    for (filter, address, u, anisotropy, expected) in [
        (
            Filter::Nearest,
            AddressMode::Repeat,
            1.25f32,
            1,
            [255u32, 0, 0, 255],
        ),
        (
            Filter::Nearest,
            AddressMode::MirrorRepeat,
            1.25,
            1,
            [0, 0, 255, 255],
        ),
        (
            Filter::Nearest,
            AddressMode::ClampToEdge,
            1.25,
            1,
            [0, 0, 255, 255],
        ),
        (
            Filter::Linear,
            AddressMode::ClampToEdge,
            0.5,
            1,
            [128, 0, 128, 255],
        ),
        (
            Filter::Linear,
            AddressMode::ClampToEdge,
            0.5,
            16,
            [128, 0, 128, 255],
        ),
    ] {
        let sampler = gpu
            .sampler(&SamplerDesc {
                min: filter,
                mag: filter,
                mip: filter,
                address: [address; 3],
                anisotropy,
                ..Default::default()
            })?
            .index;
        let mut rec = gpu.begin(Queue::Graphics);
        rec.barriers([edge(
            Resource::Buffer(output),
            Access::HostRead,
            Access::Storage,
        )]);
        rec.push(Command::Pipeline(p));
        rec.push(Command::Constants(RootConstants {
            words: [image_slot, output_slot, sampler, u.to_bits(), 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
        rec.barriers([edge(
            Resource::Buffer(output),
            Access::Storage,
            Access::HostRead,
        )]);
        let point = gpu.submit(rec.finish(), &[])?;
        gpu.wait(point)?;
        assert_eq!(
            gpu.read(output)?,
            bytemuck::cast_slice::<u32, u8>(&expected),
            "{filter:?} {address:?} anisotropy={anisotropy}"
        );
        gpu.assert_clean()?;
    }
    Ok(())
}
#[test]
fn per_target_blending_preserves_independent_equations() -> Result<()> {
    let mut gpu = device("mrt")?;
    let image = || TextureDesc {
        image: ImageDesc {
            size: [8, 8],
            format: Format::Rgba8,
            sampled: false,
            storage: false,
            attachment: true,
            copy_source: true,
            copy_destination: false,
        },
        shape: ImageShape::default(),
    };
    let targets = [gpu.create_texture(&image())?, gpu.create_texture(&image())?];
    let outputs = [
        gpu.create_readback_buffer(256)?,
        gpu.create_readback_buffer(256)?,
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/feature_cooked");
    let mut metadata: CookedPipeline =
        serde_json::from_slice(&std::fs::read(dir.join("dual.json"))?)?;
    let add = BlendComponent {
        source: BlendFactor::One,
        destination: BlendFactor::One,
        operation: BlendOp::Add,
    };
    metadata.state.raster.blends = vec![
        Some(BlendState {
            color: add,
            alpha: add,
        }),
        None,
    ];
    let p = gpu.create_pipeline(&metadata, &dir)?;
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barriers(
        targets.map(|image| edge(Resource::Image(image), Access::Undefined, Access::Color)),
    );
    rec.push(Command::BeginRendering {
        colors: targets
            .map(|image| Attachment {
                image,
                clear: Some([0., 0., 1., 1.]),
            })
            .to_vec(),
        depth: None,
        size: [8, 8],
    });
    rec.push(Command::Pipeline(p));
    rec.push(Command::Fullscreen);
    rec.push(Command::EndRendering);
    for (image, output) in targets.into_iter().zip(outputs) {
        rec.barriers([edge(
            Resource::Image(image),
            Access::Color,
            Access::CopySource,
        )]);
        rec.push(Command::ReadImage {
            source: image,
            destination: output,
        });
        rec.barriers([edge(
            Resource::Buffer(output),
            Access::CopyDestination,
            Access::HostRead,
        )]);
    }
    let point = gpu.submit(rec.finish(), &[])?;
    gpu.wait(point)?;
    for (output, expected) in outputs
        .into_iter()
        .zip([[255, 0, 255, 255], [255, 0, 0, 255]])
    {
        assert!(
            gpu.read(output)?
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == expected)
        );
    }
    gpu.assert_clean()?;
    Ok(())
}

#[test]
fn sampled_and_storage_views_select_the_same_array_mip_and_layers() -> Result<()> {
    let mut gpu = device("views")?;
    let desc = || TextureDesc {
        image: ImageDesc {
            size: [8, 8],
            format: Format::Rgba32Uint,
            sampled: true,
            storage: true,
            attachment: false,
            copy_source: true,
            copy_destination: true,
        },
        shape: ImageShape {
            layers: 4,
            mip_levels: 4,
            ..Default::default()
        },
    };
    let a = gpu.create_texture(&desc())?;
    let b = gpu.create_texture(&desc())?;
    let view = ImageViewDesc {
        dimension: ViewDimension::D2Array,
        base_mip: 1,
        mip_count: 1,
        base_layer: 1,
        layer_count: 2,
    };
    let source = gpu.sampled_view(a, view)?.index;
    let target = gpu.storage_view(b, view)?.index;
    let p = pipeline(&mut gpu, "views")?;
    let input = gpu.create_buffer(160, ara_gpu::vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let output = gpu.create_readback_buffer(160)?;
    gpu.upload(input, &[0x37; 160])?;
    let region = ImageRegion {
        mip: 1,
        origin: [0, 0, 0],
        extent: [2, 2, 1],
        base_layer: 1,
        layer_count: 2,
    };
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barriers([
        edge(
            Resource::Buffer(input),
            Access::HostWrite,
            Access::CopySource,
        ),
        edge(
            Resource::Image(a),
            Access::Undefined,
            Access::CopyDestination,
        ),
        edge(Resource::Image(b), Access::Undefined, Access::Storage),
    ]);
    rec.push(Command::FillBuffer {
        destination: output,
        offset: 0,
        size: 160,
        value: 0,
    });
    // RGBA32 texels require a 16-byte-aligned offset.
    rec.push(Command::UploadImageRegion {
        source: input,
        destination: a,
        buffer_offset: 16,
        region,
    });
    rec.barriers([edge(
        Resource::Image(a),
        Access::CopyDestination,
        Access::ShaderRead,
    )]);
    rec.push(Command::Pipeline(p));
    rec.push(Command::Constants(RootConstants {
        words: [source, target, 0, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Dispatch([2, 2, 2]));
    rec.barriers([
        edge(Resource::Image(b), Access::Storage, Access::CopySource),
        edge(
            Resource::Buffer(output),
            Access::CopyDestination,
            Access::CopyDestination,
        ),
    ]);
    rec.push(Command::ReadImageRegion {
        source: b,
        destination: output,
        buffer_offset: 16,
        region,
    });
    rec.barriers([edge(
        Resource::Buffer(output),
        Access::CopyDestination,
        Access::HostRead,
    )]);
    let p = gpu.submit(rec.finish(), &[])?;
    gpu.wait(p)?;
    let bytes = gpu.read(output)?;
    assert_eq!(&bytes[..16], &[0; 16]);
    assert_eq!(&bytes[16..144], &[0x37; 128]);
    assert_eq!(&bytes[144..], &[0; 16]);
    gpu.assert_clean()?;
    Ok(())
}
