//! The portable block-compressed formats (`Format::Bc7`, `Bc7Srgb`, `Bc5`): a layered mip
//! chain uploads as shipped and reads back byte for byte, on either backend.
use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, GpuDevice, Queue, Resource,
    commands::Command,
    descriptors::{Format, ImageDesc},
    texture::{ImageRegion, ImageShape, TextureDesc},
    vk,
};

fn barrier(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}

/// An 8x4 chain to 1x1 in two layers: levels of 2, 1, 1 and 1 blocks a layer.
fn chain(format: Format) -> TextureDesc {
    TextureDesc {
        image: ImageDesc {
            size: [8, 4],
            format,
            sampled: true,
            storage: false,
            attachment: false,
            copy_source: true,
            copy_destination: true,
        },
        shape: ImageShape {
            layers: 2,
            mip_levels: 4,
            ..Default::default()
        },
    }
}

/// Block formats are sized in whole 4x4 blocks of 16 bytes, also under 4x4 texels.
#[test]
fn block_formats_are_sized_in_whole_blocks() -> Result<()> {
    for format in [Format::Bc7, Format::Bc7Srgb, Format::Bc5] {
        assert_eq!(format.block(), (4, 16));
        assert_eq!(chain(format).copy_byte_len()?, (2 + 1 + 1 + 1) * 16 * 2);
    }
    assert_eq!(Format::Rgba8.block(), (1, 4));
    assert_eq!(Format::Rgba16Float.block(), (1, 8));
    Ok(())
}

/// Every level and layer is written whole (`UploadImage`), then the 2x1 level of the
/// second layer alone is replaced (`UploadImageRegion` at its true texel extent, one
/// block of bytes), and the image reads back as the bytes written. Where the device does
/// not sample BC textures there is nothing to check: the test says so and stops.
#[test]
fn block_compressed_chains_round_trip_in_layers() -> Result<()> {
    let mut gpu = GpuDevice::new_raster(
        &[],
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/block-formats"),
    )?;
    if !gpu.info().texture_compression_bc {
        eprintln!("skipped: the device does not sample BC textures");
        return Ok(());
    }
    for format in [Format::Bc7, Format::Bc7Srgb, Format::Bc5] {
        let desc = chain(format);
        let bytes = desc.copy_byte_len()?;
        let image = gpu.create_texture(&desc)?;
        let input = gpu.create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
        let patch = gpu.create_buffer(16, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
        let output = gpu.create_readback_buffer(bytes)?;
        let mut expected: Vec<u8> = (0..bytes).map(|i| (i * 7 % 251) as u8).collect();
        gpu.upload(input, &expected)?;
        gpu.upload(patch, &[0xa5; 16])?;
        gpu.upload(output, &vec![0; bytes as usize])?;
        // Whole-image order: level by level, each level's layers one after the other.
        // Level 2 (2x1 texels) starts after levels 0 (2 blocks a layer) and 1 (1 block).
        let level_two_layer_one = (2 + 1) * 16 * 2 + 16;
        expected[level_two_layer_one..level_two_layer_one + 16].fill(0xa5);
        let mut rec = gpu.begin(Queue::Graphics);
        rec.barriers([
            barrier(
                Resource::Buffer(input),
                Access::HostWrite,
                Access::CopySource,
            ),
            barrier(
                Resource::Buffer(patch),
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
            source: input,
            destination: image,
        });
        rec.barriers([barrier(
            Resource::Image(image),
            Access::CopyDestination,
            Access::CopyDestination,
        )]);
        rec.push(Command::UploadImageRegion {
            source: patch,
            destination: image,
            buffer_offset: 0,
            region: ImageRegion {
                mip: 2,
                origin: [0; 3],
                extent: [2, 1, 1],
                base_layer: 1,
                layer_count: 1,
            },
        });
        rec.barriers([barrier(
            Resource::Image(image),
            Access::CopyDestination,
            Access::CopySource,
        )]);
        rec.push(Command::ReadImage {
            source: image,
            destination: output,
        });
        rec.barriers([barrier(
            Resource::Buffer(output),
            Access::CopyDestination,
            Access::HostRead,
        )]);
        let point = gpu.submit(rec.finish(), &[])?;
        gpu.wait(point)?;
        assert_eq!(gpu.read(output)?, expected, "{format:?}");
    }
    gpu.assert_clean()?;
    Ok(())
}
