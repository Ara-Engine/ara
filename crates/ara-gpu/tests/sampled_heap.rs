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
        indices.push(gpu.sampled_image(image, false).unwrap().index);
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
        words: [input_slot, output_slot, 257, 0, 0, 0, 0, 0],
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

    let mut commands = gpu.begin(Queue::Graphics);
    commands.barriers([edge(
        Resource::Buffer(output),
        Access::HostRead,
        Access::Storage,
    )]);
    commands.push(Command::Pipeline(pipeline));
    commands.push(Command::Constants(RootConstants {
        words: [input_slot, output_slot, 1, 0.5f32.to_bits(), 256, 0, 0, 0],
    }));
    commands.push(Command::Dispatch([1, 1, 1]));
    commands.barriers([edge(
        Resource::Buffer(output),
        Access::Storage,
        Access::HostRead,
    )]);
    let point = gpu.submit(commands.finish(), &[]).unwrap();
    gpu.resolve(point).unwrap();
    assert_eq!(&gpu.read(output).unwrap()[..4], &[128, 0, 128, 255]);
    for resource in resources {
        gpu.retire(resource).unwrap();
    }
    gpu.retire_pipeline(pipeline).unwrap();
    gpu.collect().unwrap();
    gpu.assert_clean().unwrap();
}
