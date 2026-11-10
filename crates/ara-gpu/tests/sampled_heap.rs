#![allow(clippy::unwrap_used)]
//! GPU reads above the old descriptor limit; native mips use the material sampler.
use ara_gpu::{
    Access, Barrier, Device, Queue, Resource, RootConstants,
    cooked::CookedPipeline,
    vk,
    vulkan::{Command, VulkanDevice},
};
use std::path::Path;

fn edge(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn variable_heap_samples_high_slots_and_filters_native_mips() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join("../../target/ara-gpu/sampled-heap")).unwrap();
    let capacity = gpu.context().info.sampled_image_capacity;
    if capacity < 257 {
        eprintln!("sampled heap capacity {capacity}; this probe needs 257 slots");
        return;
    }
    let cooked = root.join("tests/cooked");
    let metadata: CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("sampled_heap.json")).unwrap()).unwrap();
    let pipeline = gpu.create_pipeline(&metadata, &cooked).unwrap();
    let mut commands = gpu.begin(Queue::Graphics);
    let mut resources = Vec::new();
    // The same runtime buffer binding must really address slots above 63.
    let buffer_capacity = gpu.context().info.storage_buffer_capacity;
    let reserved = buffer_capacity.saturating_sub(2).min(1025);
    for _ in 0..reserved {
        let buffer = gpu
            .create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)
            .unwrap();
        gpu.storage_buffer(buffer).unwrap();
        resources.push(Resource::Buffer(buffer));
    }
    let mut indices = Vec::new();
    let mut expected = Vec::new();
    for i in 0..257u32 {
        let color = if i == 256 {
            [255, 0, 0, 255]
        } else {
            [i as u8, (i * 17) as u8, (255 - i) as u8, 255]
        };
        let (size, mips, bytes) = if i == 256 {
            // Red finest level, blue coarse level: LOD 0.5 must yield purple.
            ([2, 2], 2, [color.repeat(4), vec![0, 0, 255, 255]].concat())
        } else {
            ([1, 1], 1, color.to_vec())
        };
        let image = gpu
            .create_mip_image(
                size,
                vk::Format::R8G8B8A8_UNORM,
                mips,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            )
            .unwrap();
        let stage = gpu
            .create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)
            .unwrap();
        gpu.upload(stage, &bytes).unwrap();
        commands.barriers([
            edge(
                Resource::Buffer(stage),
                Access::HostWrite,
                Access::CopySource,
            ),
            edge(
                Resource::Image(image),
                Access::Undefined,
                Access::CopyDestination,
            ),
        ]);
        commands.push(Command::UploadImage {
            source: stage,
            destination: image,
        });
        commands.barriers([edge(
            Resource::Image(image),
            Access::CopyDestination,
            Access::ShaderRead,
        )]);
        indices.push(
            gpu.sampled_view(
                image,
                ara_gpu::texture::ImageViewDesc {
                    dimension: ara_gpu::texture::ViewDimension::D2,
                    base_mip: 0,
                    mip_count: mips,
                    base_layer: 0,
                    layer_count: 1,
                },
            )
            .unwrap()
            .index,
        );
        expected.extend(color);
        resources.extend([Resource::Image(image), Resource::Buffer(stage)]);
    }
    assert!(
        indices[256] > 127,
        "must actually address the runtime array's high slots"
    );
    let input = gpu
        .create_buffer(
            indices.len() as u64 * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )
        .unwrap();
    let output = gpu
        .create_buffer(
            expected.len() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )
        .unwrap();
    gpu.upload(input, bytemuck::cast_slice(&indices)).unwrap();
    let input_slot = gpu.storage_buffer(input).unwrap().index;
    let output_slot = gpu.storage_buffer(output).unwrap().index;
    eprintln!("storage-buffer capacity {buffer_capacity}; shader output slot {output_slot}");
    if buffer_capacity > 64 {
        assert!(
            output_slot > 63,
            "shader must write above the old buffer ceiling"
        );
    }
    resources.extend([Resource::Buffer(input), Resource::Buffer(output)]);
    commands.barriers([
        edge(
            Resource::Buffer(input),
            Access::HostWrite,
            Access::ShaderRead,
        ),
        edge(Resource::Buffer(output), Access::Undefined, Access::Storage),
    ]);
    commands.push(Command::Pipeline(pipeline));
    commands.push(Command::Constants(RootConstants {
        words: [input_slot, output_slot, 257, 0, 0, 2, 0, 0],
    }));
    commands.push(Command::BeginPass("Heap.SampleHighSlots".into()));
    commands.push(Command::Dispatch([257u32.div_ceil(64), 1, 1]));
    commands.push(Command::EndPass);
    commands.barriers([edge(
        Resource::Buffer(output),
        Access::Storage,
        Access::HostRead,
    )]);
    let point = gpu.submit(commands.finish(), &[]).unwrap();
    // Publish a fresh descriptor without waiting for the already-bound set.
    let extra = gpu
        .create_image(
            1,
            1,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::SAMPLED,
        )
        .unwrap();
    assert!(gpu.sampled_image(extra, false).unwrap().index > 256);
    resources.push(Resource::Image(extra));
    let timings = gpu.resolve(point).unwrap();
    assert_eq!(gpu.read(output).unwrap(), expected);
    eprintln!(
        "{capacity} sampled slots; high-slot probe: {} ms",
        timings[0].milliseconds
    );

    use ara_gpu::sampler::{AddressMode, Filter, SamplerDesc};
    for (filter, address, expected_pixel) in [
        (Filter::Linear, AddressMode::ClampToEdge, [128, 0, 128, 255]),
        (Filter::Nearest, AddressMode::Repeat, [0, 0, 255, 255]),
        (
            Filter::Linear,
            AddressMode::MirrorRepeat,
            [128, 0, 128, 255],
        ),
    ] {
        let sampler = gpu
            .sampler(&SamplerDesc {
                min: filter,
                mag: filter,
                mip: filter,
                address: [address; 3],
                ..Default::default()
            })
            .unwrap();
        let mut commands = gpu.begin(Queue::Graphics);
        commands.barriers([edge(
            Resource::Buffer(output),
            Access::HostRead,
            Access::Storage,
        )]);
        commands.push(Command::Pipeline(pipeline));
        commands.push(Command::Constants(RootConstants {
            words: [
                input_slot,
                output_slot,
                1,
                0.5f32.to_bits(),
                256,
                sampler.index,
                0,
                0,
            ],
        }));
        commands.push(Command::Dispatch([1, 1, 1]));
        commands.barriers([edge(
            Resource::Buffer(output),
            Access::Storage,
            Access::HostRead,
        )]);
        let point = gpu.submit(commands.finish(), &[]).unwrap();
        gpu.resolve(point).unwrap();
        assert_eq!(&gpu.read(output).unwrap()[..4], &expected_pixel);
    }
    for resource in resources {
        gpu.retire(resource).unwrap();
    }
    gpu.retire_pipeline(pipeline).unwrap();
    gpu.collect().unwrap();
    assert_eq!(gpu.descriptor_usage().storage_buffers, 0);
    let reused = gpu
        .create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)
        .unwrap();
    assert!(gpu.storage_buffer(reused).unwrap().index < buffer_capacity);
    gpu.retire(Resource::Buffer(reused)).unwrap();
    gpu.collect().unwrap();
    assert_eq!(gpu.descriptor_usage().storage_buffers, 0);
    gpu.assert_clean().unwrap();
}
