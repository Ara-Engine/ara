#![allow(clippy::unwrap_used)]
//! Native BC chains upload and read back without pixel expansion or mip atlases.
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource, vk,
    vulkan::{Command, VulkanDevice},
};

#[test]
fn compressed_mip_chains_round_trip_and_reject_short_staging() {
    let cache =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/native-mips");
    let mut gpu = VulkanDevice::new(&[], cache).unwrap();
    for (format, size, block_bytes) in [
        (vk::Format::BC7_SRGB_BLOCK, [8u32, 4], 16u32),
        (vk::Format::BC5_UNORM_BLOCK, [3, 5], 16),
        (vk::Format::BC4_UNORM_BLOCK, [4, 4], 8),
    ] {
        let mips = size[0].max(size[1]).ilog2() + 1;
        let mut payload = Vec::new();
        for mip in 0..mips {
            let [w, h] = size.map(|v| (v >> mip).max(1));
            let bytes = w.div_ceil(4) * h.div_ceil(4) * block_bytes;
            payload.extend(std::iter::repeat_n((mip * 51 + 7) as u8, bytes as usize));
        }
        let image = gpu
            .create_mip_image(
                size,
                format,
                mips,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST,
            )
            .unwrap();
        let short = gpu
            .create_buffer(
                payload.len() as u64 - 4,
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
            )
            .unwrap();
        let mut rejected = gpu.begin(Queue::Graphics);
        rejected.push(Command::UploadImage {
            source: short,
            destination: image,
        });
        assert!(
            gpu.submit(rejected.finish(), &[]).is_err(),
            "all compressed mips must fit staging"
        );
        gpu.retire(Resource::Buffer(short)).unwrap();

        let stage = gpu
            .create_buffer(
                payload.len() as u64,
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
            )
            .unwrap();
        let readback = gpu
            .create_buffer(
                payload.len() as u64,
                vk::BufferUsageFlags::TRANSFER_DST,
                true,
            )
            .unwrap();
        gpu.upload(stage, &payload).unwrap();
        let mut commands = gpu.begin(Queue::Graphics);
        commands.barriers([
            Barrier {
                resource: Resource::Buffer(stage),
                before: Access::HostWrite,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Image(image),
                before: Access::Undefined,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        commands.push(Command::UploadImage {
            source: stage,
            destination: image,
        });
        commands.barriers([
            Barrier {
                resource: Resource::Image(image),
                before: Access::CopyDestination,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(readback),
                before: Access::Undefined,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        commands.push(Command::ReadImage {
            source: image,
            destination: readback,
        });
        commands.barriers([Barrier {
            resource: Resource::Buffer(readback),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        let point = gpu.submit(commands.finish(), &[]).unwrap();
        gpu.resolve(point).unwrap();
        assert_eq!(
            gpu.read(readback).unwrap(),
            payload,
            "exact blocks at every mip, {format:?}"
        );
        for resource in [
            Resource::Image(image),
            Resource::Buffer(stage),
            Resource::Buffer(readback),
        ] {
            gpu.retire(resource).unwrap();
        }
        gpu.collect().unwrap();
    }
    assert!(
        gpu.create_mip_image(
            [8, 8],
            vk::Format::BC7_UNORM_BLOCK,
            5,
            vk::ImageUsageFlags::SAMPLED
        )
        .is_err()
    );
    gpu.assert_clean().unwrap();
}
