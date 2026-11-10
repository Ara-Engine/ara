//! Heap sharing and completion-gated range reuse, plus an explicit footprint probe.
use super::*;
use crate::commands::Command;
mod mixed;
#[test]
fn private_resources_share_heaps_and_retired_ranges_wait_for_gpu_completion() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let usage = vk::BufferUsageFlags::TRANSFER_SRC
        | vk::BufferUsageFlags::TRANSFER_DST
        | vk::BufferUsageFlags::STORAGE_BUFFER;
    let a = gpu.create_buffer(4096, usage, false).unwrap();
    let b = gpu.create_buffer(4096, usage, false).unwrap();
    let (heap, address) = {
        let raw = &gpu.buffers.get(a).unwrap().raw;
        (raw.heap.clone().unwrap(), raw.buffer.gpuAddress())
    };
    assert!(std::ptr::eq(
        &*heap,
        &**gpu.buffers.get(b).unwrap().raw.heap.as_ref().unwrap()
    ));
    let image = gpu
        .create_image(
            16,
            16,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::SAMPLED,
        )
        .unwrap();
    assert!(gpu.images.get(image).unwrap().raw.heap.is_some());
    let out = gpu.create_readback_buffer(4096).unwrap();
    let stall = gpu.device.newSharedEvent().unwrap();
    gpu.wait_event(&stall, 1);
    gpu.begin_deferred_completion().unwrap();
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::FillBuffer {
        destination: a,
        offset: 0,
        size: 4096,
        value: 0x12345678,
    });
    rec.barriers([Barrier {
        resource: Resource::Buffer(a),
        before: Access::CopyDestination,
        after: Access::CopySource,
        handoff: None,
    }]);
    rec.push(Command::CopyBuffer {
        source: a,
        destination: out,
        size: 4096,
    });
    gpu.submit(rec.finish(), &[]).unwrap();
    let mut frame = gpu.end_deferred_completion().unwrap();
    Device::retire(&mut gpu, Resource::Buffer(a)).unwrap();
    Device::collect(&mut gpu).unwrap();
    let c = gpu.create_buffer(4096, usage, false).unwrap();
    assert_ne!(gpu.buffers.get(c).unwrap().raw.buffer.gpuAddress(), address);
    stall.setSignaledValue(1);
    gpu.wait_completion(&mut frame).unwrap();
    assert!(
        gpu.read(out)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|b| *b == 0x12345678u32.to_le_bytes())
    );
    Device::retire(&mut gpu, Resource::Buffer(b)).unwrap();
    Device::retire(&mut gpu, Resource::Buffer(c)).unwrap();
    Device::retire(&mut gpu, Resource::Image(image)).unwrap();
    Device::collect(&mut gpu).unwrap();
    assert_eq!(heap.usedSize(), 0);
}
#[test]
#[ignore = "allocation footprint; run through ara-dev gpu"]
fn allocation_footprint() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let sizes: Vec<_> = (0..4096)
        .map(|i| 129000 + (i * 7919 % 12000) as u64)
        .collect();
    let requested: u64 = sizes.iter().sum();
    let mut raw = Vec::new();
    for size in &sizes {
        raw.push(
            resources::buffer(
                &gpu.device,
                *size,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                false,
            )
            .unwrap(),
        );
    }
    let baseline: u64 = raw
        .iter()
        .map(|b| MTLAllocation::allocatedSize(&*b.buffer) as u64)
        .sum();
    drop(raw);
    let raw_texture = resources::image(
        &gpu.device,
        [64, 64],
        vk::Format::R8G8B8A8_UNORM,
        1,
        vk::ImageUsageFlags::SAMPLED,
    )
    .unwrap();
    drop(raw_texture);
    let mut handles = Vec::new();
    for size in &sizes {
        handles.push(
            gpu.create_buffer(*size, vk::BufferUsageFlags::STORAGE_BUFFER, false)
                .unwrap(),
        );
    }
    let mut heaps = Vec::new();
    for h in &handles {
        let heap = gpu.buffers.get(*h).unwrap().raw.heap.as_ref().unwrap();
        if !heaps
            .iter()
            .any(|h: &memory::Heap| std::ptr::eq(&**h, &**heap))
        {
            heaps.push(heap.clone());
        }
    }
    let pooled: u64 = heaps.iter().map(|h| h.size() as u64).sum();
    eprintln!(
        "allocation requested_bytes={requested} unpooled_bytes={baseline} pooled_bytes={pooled} heaps={}",
        heaps.len()
    );
}
