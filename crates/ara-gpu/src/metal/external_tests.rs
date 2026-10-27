//! Shared surfaces between two Metal devices in one process (the cross-process lookup
//! of global IOSurface IDs is the same call; see `metal::external`).
use super::external::{ExportRing, ImportedSurface};
use super::*;
use crate::commands::{Attachment, Command};
use crate::shared::SharedSurface;

fn device() -> MetalDevice {
    MetalDevice::new_raster(&[], std::env::temp_dir()).expect("Metal 4 device")
}

/// A completed RGBA8 transfer source cleared to `color` on `gpu`.
fn cleared(gpu: &mut MetalDevice, size: [u32; 2], color: [f32; 4]) -> Image {
    let image = gpu
        .create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("source");
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image,
            clear: Some(color),
        }],
        depth: None,
        size,
    });
    rec.push(Command::EndRendering);
    gpu.submit(rec.finish(), &[]).expect("clear");
    image
}

#[test]
fn shared_surfaces_round_trip_between_devices_skip_when_full_and_resize() {
    let (mut producer, owner) = (device(), device());
    let size = [16, 8];
    // SAFETY: both devices outlive the ring and import; this thread owns their queues.
    let mut ring = unsafe { ExportRing::create(&producer, size) }.expect("ring");
    let source = cleared(&mut producer, size, [1., 0., 0., 1.]);
    // SAFETY: `source` is a completed RGBA8 image of `size` on `producer`.
    let publish = |ring: &mut ExportRing, producer: &MetalDevice, size, tick| unsafe {
        ring.publish_image(producer, source, size, tick)
            .expect("publish")
    };
    let frame = publish(&mut ring, &producer, size, 7).expect("a free slot");
    let desc = ring.desc();
    assert_eq!(
        (desc.generation, desc.slots.len()),
        (1, crate::shared::SLOTS)
    );
    assert_eq!(
        (desc.width, desc.height, desc.process),
        (16, 8, std::process::id())
    );
    let shared = SharedSurface::duplicate_local(&desc).expect("IOSurface lookup");
    // SAFETY: as for the ring.
    let mut imported = unsafe { ImportedSurface::create(&owner, &shared) }.expect("import");
    // SAFETY: the producer published `frame`; the owner has not used the slot.
    unsafe { imported.exchange(Some(frame.slot), None) }.expect("acquire");
    // SAFETY: this thread owns the import's queue.
    let pixels = unsafe { imported.read_pixels(&owner, frame.slot) }.expect("read");
    let pixels: &[u32] = bytemuck::cast_slice(&pixels);
    assert!(
        pixels.iter().all(|&p| p == 0xff00_00ff),
        "{:08x}",
        pixels[0]
    );
    assert!(
        imported.texture(frame.slot).is_some(),
        "native slot texture"
    );
    // SAFETY: the readback above completed.
    unsafe { imported.exchange(None, Some(frame.slot)) }.expect("release");
    // SAFETY: as above; an unowned release is skipped, as on Vulkan.
    unsafe { imported.exchange(None, Some(frame.slot)) }.expect("skipped release");
    assert!(!imported.owns(frame.slot));
    assert!(ring.release(frame));
    // While the owner holds every slot the producer skips frames instead of blocking.
    let held: Vec<_> = (0..crate::shared::SLOTS)
        .map(|tick| publish(&mut ring, &producer, size, tick as u64).expect("free slot"))
        .collect();
    assert!(publish(&mut ring, &producer, size, 9).is_none());
    // A resize starts a new generation with new surfaces; old releases are stale, and
    // a source still at the old size is skipped until the renderer catches up.
    // SAFETY: as for `create`.
    unsafe { ring.recreate(&producer, [32, 16]) }.expect("resize");
    let resized = ring.desc();
    assert_eq!(resized.generation, 2);
    assert!(
        resized
            .slots
            .iter()
            .all(|slot| desc.slots.iter().all(|old| old.handle != slot.handle))
    );
    assert!(
        !ring.release(held[0]),
        "a stale generation's release is ignored"
    );
    assert!(
        publish(&mut ring, &producer, size, 10).is_none(),
        "mid-resize frame"
    );
}

#[test]
fn imports_reject_foreign_processes_and_missing_surfaces() {
    let producer = device();
    // SAFETY: the device outlives the ring; this thread owns its queue.
    let ring = unsafe { ExportRing::create(&producer, [4, 4]) }.expect("ring");
    let mut desc = ring.desc();
    desc.process += 1;
    assert!(
        SharedSurface::duplicate_local(&desc).is_err(),
        "another process's surfaces"
    );
    let mut desc = ring.desc();
    desc.slots[0].size += 1;
    assert!(
        SharedSurface::duplicate_local(&desc).is_err(),
        "a mismatched allocation"
    );
}
