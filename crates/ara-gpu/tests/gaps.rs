//! Backend-neutral probes for Windows Vulkan verification and Mac Metal parity.
use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, GpuDevice, Queue, Resource,
    commands::{Attachment, Command},
    descriptors::{Format, ImageDesc},
    texture::{ImageRegion, ImageShape, SampleCount, TextureDesc},
};
fn device(name: &str) -> Result<GpuDevice> {
    Ok(GpuDevice::new_raster(
        &[],
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-gpu/gaps")
            .join(name),
    )?)
}
fn barrier(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
#[test]
fn large_updates_keep_both_in_flight_payloads_and_fill_patterns() -> Result<()> {
    let mut gpu = device("uploads")?;
    let size = 1024 * 1024;
    let target = gpu.create_buffer(size, vk_usage(), false)?;
    let output = gpu.create_readback_buffer(size * 2)?;
    gpu.begin_deferred_completion()?;
    for (i, value) in [17, 29].into_iter().enumerate() {
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::FillBuffer {
            destination: target,
            offset: 0,
            size,
            value: 0x12345678,
        });
        rec.push(Command::UpdateBuffer {
            destination: target,
            offset: 4,
            bytes: vec![value; size as usize - 8],
        });
        rec.barriers([barrier(
            Resource::Buffer(target),
            Access::CopyDestination,
            Access::CopySource,
        )]);
        rec.push(Command::CopyBufferRange {
            source: target,
            source_offset: 0,
            destination: output,
            destination_offset: i as u64 * size,
            size,
        });
        rec.barriers([barrier(
            Resource::Buffer(target),
            Access::CopySource,
            Access::CopyDestination,
        )]);
        gpu.submit(rec.finish(), &[])?;
    }
    let mut frame = gpu.end_deferred_completion()?;
    gpu.wait_completion(&mut frame)?;
    let bytes = gpu.read(output)?;
    for (part, value) in bytes.chunks(size as usize).zip([17, 29]) {
        assert_eq!(&part[..4], &0x12345678u32.to_le_bytes());
        assert!(part[4..part.len() - 4].iter().all(|b| *b == value));
        assert_eq!(&part[part.len() - 4..], &0x12345678u32.to_le_bytes());
    }
    gpu.assert_clean()?;
    Ok(())
}
fn vk_usage() -> ara_gpu::vk::BufferUsageFlags {
    ara_gpu::vk::BufferUsageFlags::TRANSFER_SRC | ara_gpu::vk::BufferUsageFlags::TRANSFER_DST
}
#[test]
fn array_regions_and_mips_roundtrip_with_buffer_offsets() -> Result<()> {
    let mut gpu = device("regions")?;
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
    let input = gpu.create_buffer(160, ara_gpu::vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let output = gpu.create_readback_buffer(160)?;
    gpu.upload(input, &[0x37; 160])?;
    let region = ImageRegion {
        mip: 1,
        origin: [1, 1, 0],
        extent: [2, 2, 1],
        base_layer: 1,
        layer_count: 2,
    };
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barriers([
        barrier(
            Resource::Buffer(input),
            Access::HostWrite,
            Access::CopySource,
        ),
        barrier(
            Resource::Image(a),
            Access::Undefined,
            Access::CopyDestination,
        ),
        barrier(
            Resource::Image(b),
            Access::Undefined,
            Access::CopyDestination,
        ),
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
    rec.barriers([barrier(
        Resource::Image(a),
        Access::CopyDestination,
        Access::CopySource,
    )]);
    rec.push(Command::CopyImageRegion {
        source: a,
        destination: b,
        from: region,
        to: region,
    });
    rec.barriers([
        barrier(
            Resource::Image(b),
            Access::CopyDestination,
            Access::CopySource,
        ),
        barrier(
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
    rec.barriers([barrier(
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
#[test]
fn multisample_clear_resolves_to_single_sample_pixels() -> Result<()> {
    let mut gpu = device("resolve")?;
    let desc = |samples| TextureDesc {
        image: ImageDesc {
            size: [8, 8],
            format: Format::Rgba8,
            sampled: false,
            storage: false,
            attachment: true,
            copy_source: true,
            copy_destination: true,
        },
        shape: ImageShape {
            samples,
            ..Default::default()
        },
    };
    let source = gpu.create_texture(&desc(SampleCount::Four))?;
    let target = gpu.create_texture(&desc(SampleCount::One))?;
    let output = gpu.create_readback_buffer(256)?;
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barriers([
        barrier(Resource::Image(source), Access::Undefined, Access::Color),
        barrier(
            Resource::Image(target),
            Access::Undefined,
            Access::CopyDestination,
        ),
    ]);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: source,
            clear: Some([0.25, 0.5, 0.75, 1.]),
        }],
        depth: None,
        size: [8, 8],
    });
    rec.push(Command::EndRendering);
    rec.barriers([barrier(
        Resource::Image(source),
        Access::Color,
        Access::CopySource,
    )]);
    rec.push(Command::ResolveImage {
        source,
        destination: target,
    });
    rec.barriers([barrier(
        Resource::Image(target),
        Access::CopyDestination,
        Access::CopySource,
    )]);
    rec.push(Command::ReadImage {
        source: target,
        destination: output,
    });
    rec.barriers([barrier(
        Resource::Buffer(output),
        Access::CopyDestination,
        Access::HostRead,
    )]);
    let p = gpu.submit(rec.finish(), &[])?;
    gpu.wait(p)?;
    assert!(
        gpu.read(output)?
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [64, 128, 191, 255])
    );
    gpu.assert_clean()?;
    Ok(())
}

#[test]
fn batched_subresource_ranges_touch_one_array_mip_and_reject_overflow() -> Result<()> {
    use ara_gpu::sync::{BarrierRange, ResourceBarrier};
    let mut gpu = device("barrier-ranges")?;
    let image = gpu.create_texture(&TextureDesc {
        image: ImageDesc {
            size: [4, 4],
            format: Format::R32Uint,
            sampled: false,
            storage: false,
            attachment: false,
            copy_source: true,
            copy_destination: true,
        },
        shape: ImageShape {
            mip_levels: 3,
            layers: 2,
            ..Default::default()
        },
    })?;
    let input = gpu.create_buffer(16, ara_gpu::vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(input, &[37; 16])?;
    let output = gpu.create_readback_buffer(16)?;
    let range = BarrierRange::Image {
        base_mip: 1,
        mip_count: 1,
        base_layer: 1,
        layer_count: 1,
    };
    let region = ImageRegion {
        mip: 1,
        origin: [0; 3],
        extent: [2, 2, 1],
        base_layer: 1,
        layer_count: 1,
    };
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barrier_ranges([
        ResourceBarrier {
            barrier: barrier(
                Resource::Image(image),
                Access::Undefined,
                Access::CopyDestination,
            ),
            range,
        },
        ResourceBarrier {
            barrier: barrier(
                Resource::Buffer(input),
                Access::HostWrite,
                Access::CopySource,
            ),
            range: BarrierRange::Buffer {
                offset: 0,
                size: 16,
            },
        },
    ]);
    rec.push(Command::UploadImageRegion {
        source: input,
        destination: image,
        buffer_offset: 0,
        region,
    });
    rec.barrier_ranges([ResourceBarrier {
        barrier: barrier(
            Resource::Image(image),
            Access::CopyDestination,
            Access::CopySource,
        ),
        range,
    }]);
    rec.push(Command::ReadImageRegion {
        source: image,
        destination: output,
        buffer_offset: 0,
        region,
    });
    rec.barriers([barrier(
        Resource::Buffer(output),
        Access::CopyDestination,
        Access::HostRead,
    )]);
    let point = gpu.submit(rec.finish(), &[])?;
    gpu.wait(point)?;
    assert_eq!(gpu.read(output)?, [37; 16]);
    for invalid in [
        BarrierRange::Buffer {
            offset: u64::MAX,
            size: 8,
        },
        BarrierRange::Buffer { offset: 0, size: 0 },
    ] {
        let mut rec = gpu.begin(Queue::Graphics);
        rec.barrier_ranges([ResourceBarrier {
            barrier: barrier(
                Resource::Buffer(output),
                Access::HostRead,
                Access::CopyDestination,
            ),
            range: invalid,
        }]);
        assert!(gpu.submit(rec.finish(), &[]).is_err());
    }
    let mut rec = gpu.begin(Queue::Graphics);
    rec.barrier_ranges([ResourceBarrier {
        barrier: barrier(
            Resource::Image(image),
            Access::CopySource,
            Access::CopyDestination,
        ),
        range: BarrierRange::Image {
            base_mip: 2,
            mip_count: 2,
            base_layer: 0,
            layer_count: 1,
        },
    }]);
    assert!(gpu.submit(rec.finish(), &[]).is_err());
    gpu.assert_clean()?;
    Ok(())
}
