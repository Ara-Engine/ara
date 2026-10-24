//! Shared surfaces between two devices of this process: a ring exported on one,
//! imported on the other, pixels read back, every slot cycled through the
//! acquire/release protocol, a resize, and both devices validation-clean.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]
use anyhow::Result;
use ara_gpu::{
    shared::{Mailbox, SLOTS, SharedSurface},
    vk,
    vulkan::{
        Context, VulkanDevice,
        external::{ExportRing, ImportedSurface},
        image_barrier,
    },
};

/// Clears `image` (RGBA8, TRANSFER_SRC after the first fill) to `rgba` and leaves it
/// in TRANSFER_SRC_OPTIMAL, as the renderer leaves its output.
fn fill(gpu: &Context, image: vk::Image, rgba: [f32; 4], first: bool) -> Result<()> {
    gpu.immediate(|command| {
        let to_dst = image_barrier(
            image,
            vk::ImageAspectFlags::COLOR,
            if first {
                vk::ImageLayout::UNDEFINED
            } else {
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL
            },
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            (
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::MEMORY_READ,
            ),
            (
                vk::PipelineStageFlags2::CLEAR,
                vk::AccessFlags2::TRANSFER_WRITE,
            ),
        );
        let to_src = image_barrier(
            image,
            vk::ImageAspectFlags::COLOR,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            (
                vk::PipelineStageFlags2::CLEAR,
                vk::AccessFlags2::TRANSFER_WRITE,
            ),
            (
                vk::PipelineStageFlags2::COPY,
                vk::AccessFlags2::TRANSFER_READ,
            ),
        );
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        // SAFETY: A fresh recording command buffer on this device; the image is live.
        unsafe {
            gpu.device.cmd_pipeline_barrier2(
                command,
                &vk::DependencyInfo::default().image_memory_barriers(&[to_dst]),
            );
            gpu.device.cmd_clear_color_image(
                command,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &vk::ClearColorValue { float32: rgba },
                &[range],
            );
            gpu.device.cmd_pipeline_barrier2(
                command,
                &vk::DependencyInfo::default().image_memory_barriers(&[to_src]),
            );
        }
    })
}

fn source(gpu: &mut VulkanDevice, size: [u32; 2]) -> Result<vk::Image> {
    let image = gpu.create_image(
        size[0],
        size[1],
        vk::Format::R8G8B8A8_UNORM,
        // Sampled only because owned images always get a view.
        vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST
            | vk::ImageUsageFlags::SAMPLED,
    )?;
    Ok(gpu.native_image(image)?)
}

#[test]
fn a_ring_exported_on_one_device_is_sampled_on_another() -> Result<()> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sn/test-gpu");
    let mut producer = VulkanDevice::new(&[], &cache)?;
    let owner = VulkanDevice::new(&[], &cache)?;
    if !ara_gpu::vulkan::external::supported(producer.context()) {
        eprintln!("skipped: no VK_KHR_external_memory_win32");
        return Ok(());
    }
    let mut size = [64, 32];
    let mut image = source(&mut producer, size)?;
    // SAFETY: Both devices outlive the ring and imports; this thread owns the queues.
    let mut ring = unsafe { ExportRing::new(producer.context(), size)? };
    let mut shared = SharedSurface::duplicate_local(&ring.desc())?;
    // SAFETY: As above.
    let mut imported = unsafe { ImportedSurface::new(owner.context(), &shared)? };
    let mut mailbox = Mailbox::default();
    let mut shown = None;
    let mut first = true;
    for round in 0..(SLOTS * 3) as u32 {
        if round == 7 {
            // A resize is a new generation: new images, handles and import.
            size = [48, 40];
            image = source(&mut producer, size)?;
            first = true;
            // SAFETY: As above.
            unsafe { ring.resize(producer.context(), size)? };
            shared = SharedSurface::duplicate_local(&ring.desc())?;
            drop(imported);
            // SAFETY: As above.
            imported = unsafe { ImportedSurface::new(owner.context(), &shared)? };
            shown = None;
        }
        let value = (round * 20 + 10) as u8;
        fill(
            producer.context(),
            image,
            [
                f32::from(value) / 255.0,
                0.25,
                1.0 - f32::from(value) / 255.0,
                1.0,
            ],
            first,
        )?;
        first = false;
        // SAFETY: `image` is RGBA8 of the ring's size in TRANSFER_SRC, filled above.
        let frame = unsafe { ring.publish(image, u64::from(round))? }.expect("a free slot");
        assert_eq!(frame.generation, shared.desc().generation);
        assert!(mailbox.deliver(frame).is_none());
        let frame = mailbox.take().unwrap();
        // SAFETY: The previous frame's reads completed (every run waits).
        unsafe {
            imported.exchange(
                Some(frame.slot),
                shown.map(|f: ara_gpu::shared::Frame| f.slot),
            )?
        };
        if let Some(previous) = shown.replace(frame) {
            assert!(ring.release(previous), "release of {previous:?}");
        }
        // SAFETY: The slot is acquired by the owner device.
        let pixels = unsafe { imported.read(frame.slot)? };
        assert_eq!(pixels.len(), (size[0] * size[1] * 4) as usize);
        let expected = [value, 64, 255 - value, 255];
        for pixel in pixels.as_chunks::<4>().0 {
            for (got, want) in pixel.iter().zip(expected) {
                assert!(
                    got.abs_diff(want) <= 1,
                    "round {round}: {pixel:?} != {expected:?}"
                );
            }
        }
        assert_eq!(
            ring.free(),
            SLOTS - 1,
            "the owner holds only the shown slot"
        );
    }
    drop(imported);
    drop(ring);
    producer.assert_clean()?;
    owner.assert_clean()?;
    Ok(())
}

#[test]
fn imports_are_refused_for_another_device_identity() -> Result<()> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sn/test-gpu");
    let producer = VulkanDevice::new(&[], &cache)?;
    if !ara_gpu::vulkan::external::supported(producer.context()) {
        return Ok(());
    }
    // SAFETY: The device outlives the ring; this thread owns the queue.
    let ring = unsafe { ExportRing::new(producer.context(), [16, 16])? };
    let mut desc = ring.desc();
    desc.device.driver_uuid[0] ^= 0xff;
    let shared = SharedSurface::duplicate_local(&desc)?;
    // SAFETY: As above; the import is refused before touching any handle.
    let error = unsafe { ImportedSurface::new(producer.context(), &shared) }
        .err()
        .expect("a foreign driver is refused");
    assert!(error.to_string().starts_with("ARA_GPU_EXTERNAL"), "{error}");
    let diagnostic = ara_diag::Diagnostic::from(&error);
    assert_eq!(diagnostic.code, ara_gpu::GPU_EXTERNAL);
    Ok(())
}
