//! Native format transfer coverage.
use super::*;
use crate::commands::Command;

#[test]
fn portable_formats_roundtrip_every_mip_without_shader_conversion() {
    use crate::descriptors::Format;
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    for (format, stride) in [
        (Format::Rgba32Uint, 16),
        (Format::R32Uint, 4),
        (Format::R32Float, 4),
        (Format::R16Float, 2),
        (Format::Rg32Uint, 8),
        (Format::Rg16Float, 4),
        (Format::Depth16Unorm, 2),
    ] {
        let bytes = (8 * 4 + 4 * 2 + 2 + 1) * stride;
        let image = gpu
            .create_mip_image(
                [8, 4],
                format.vk(),
                4,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )
            .unwrap();
        gpu.sampled_image(image, format == Format::Depth16Unorm)
            .unwrap();
        let staging = gpu
            .create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_SRC, true)
            .unwrap();
        let readback = gpu.create_readback_buffer(bytes).unwrap();
        let pattern = vec![0x3c; bytes as usize];
        gpu.upload(staging, &pattern).unwrap();
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::UploadImage {
            source: staging,
            destination: image,
        });
        rec.barriers([Barrier {
            resource: Resource::Image(image),
            before: Access::CopyDestination,
            after: Access::CopySource,
            handoff: None,
        }]);
        rec.push(Command::ReadImage {
            source: image,
            destination: readback,
        });
        let point = gpu.submit(rec.finish(), &[]).unwrap();
        gpu.wait(point).unwrap();
        assert_eq!(gpu.read(readback).unwrap(), pattern, "{format:?}");
    }
}
