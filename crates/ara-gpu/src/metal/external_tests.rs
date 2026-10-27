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
    let (mut producer, mut owner) = (device(), device());
    let size = [16, 8];
    let mut ring = ExportRing::new(&mut producer, size).expect("ring");
    let source = cleared(&mut producer, size, [1., 0., 0., 1.]);
    let frame = ring
        .publish(&mut producer, source, 7)
        .expect("publish")
        .expect("a free slot");
    let desc = ring.desc(&producer);
    assert_eq!(
        (desc.generation, desc.slots.len()),
        (1, crate::shared::SLOTS)
    );
    assert_eq!(
        (desc.width, desc.height, desc.process),
        (16, 8, std::process::id())
    );
    let shared = SharedSurface::duplicate_local(&desc).expect("IOSurface lookup");
    let mut imported = ImportedSurface::new(&mut owner, &shared).expect("import");
    imported.exchange(Some(frame.slot), None).expect("acquire");
    let pixels: Vec<u32> =
        bytemuck::cast_slice(&imported.read(&mut owner, frame.slot).expect("read")).to_vec();
    assert!(
        pixels.iter().all(|&p| p == 0xff00_00ff),
        "{:08x}",
        pixels[0]
    );
    owner
        .sampled_image(imported.image(frame.slot).expect("slot image"), false)
        .expect("the owner samples the slot");
    imported.exchange(None, Some(frame.slot)).expect("release");
    assert!(
        imported.exchange(None, Some(frame.slot)).is_err(),
        "double release"
    );
    assert!(ring.release(frame));
    // While the owner holds every slot the producer skips frames instead of blocking.
    let held: Vec<_> = (0..crate::shared::SLOTS)
        .map(|tick| {
            ring.publish(&mut producer, source, tick as u64)
                .expect("publish")
                .expect("free slot")
        })
        .collect();
    assert!(
        ring.publish(&mut producer, source, 9)
            .expect("publish")
            .is_none()
    );
    // A resize starts a new generation with new surfaces; old releases are stale.
    ring.resize(&mut producer, [32, 16]).expect("resize");
    let resized = ring.desc(&producer);
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
    imported.close(&mut owner).expect("close import");
    ring.close(&mut producer).expect("close ring");
}

#[test]
fn imports_reject_foreign_processes_and_missing_surfaces() {
    let mut producer = device();
    let ring = ExportRing::new(&mut producer, [4, 4]).expect("ring");
    let mut desc = ring.desc(&producer);
    desc.process += 1;
    assert!(
        SharedSurface::duplicate_local(&desc).is_err(),
        "another process's surfaces"
    );
    let mut desc = ring.desc(&producer);
    desc.slots[0].size += 1;
    assert!(
        SharedSurface::duplicate_local(&desc).is_err(),
        "a mismatched allocation"
    );
    ring.close(&mut producer).expect("close ring");
}
