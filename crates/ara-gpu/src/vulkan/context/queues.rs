//! Prefer independent compute, while preserving a legal single-family fallback.
use anyhow::{Context, Result};
use ash::vk;
#[cfg(test)]
thread_local! {
    static SHARED_FAMILY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub(super) fn select(families: &[vk::QueueFamilyProperties]) -> Result<(u32, u32)> {
    let graphics = families
        .iter()
        .position(|f| f.queue_count > 0 && f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .context("No graphics queue")?;
    #[cfg(test)]
    if SHARED_FAMILY.get() {
        anyhow::ensure!(
            families[graphics]
                .queue_flags
                .contains(vk::QueueFlags::COMPUTE)
        );
        return Ok((graphics as u32, graphics as u32));
    }
    let compute = families
        .iter()
        .enumerate()
        .filter(|(_, f)| f.queue_count > 0 && f.queue_flags.contains(vk::QueueFlags::COMPUTE))
        .min_by_key(|(i, f)| {
            (
                f.queue_flags.contains(vk::QueueFlags::GRAPHICS),
                *i != graphics,
            )
        })
        .map(|(i, _)| i)
        .context("No compute-capable queue")?;
    Ok((graphics as u32, compute as u32))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg_attr(target_os = "macos", ignore = "Vulkan hardware probe")]
    fn shared_family_submits_both_queue_classes_on_hardware() {
        use crate::{
            Access, Barrier, Device, Queue, Resource, commands::Command, vulkan::VulkanDevice,
        };
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                SHARED_FAMILY.set(false);
            }
        }
        SHARED_FAMILY.set(true);
        let _reset = Reset;
        let mut gpu =
            VulkanDevice::new_raster(&[], std::env::temp_dir().join("ara-shared-family")).unwrap();
        assert_eq!(
            gpu.context().info.graphics_family,
            gpu.context().info.compute_family
        );
        for queue in [Queue::Graphics, Queue::Compute] {
            let buffer = gpu.create_readback_buffer(16).unwrap();
            let mut rec = gpu.begin(queue);
            rec.push(Command::FillBuffer {
                destination: buffer,
                offset: 0,
                size: 16,
                value: 0x12345678,
            });
            rec.barriers([Barrier {
                resource: Resource::Buffer(buffer),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            }]);
            let point = gpu.submit(rec.finish(), &[]).unwrap();
            gpu.wait(point).unwrap();
            assert_eq!(
                gpu.read(buffer).unwrap(),
                0x12345678u32.to_le_bytes().repeat(4)
            );
        }
        // Deferred graphics uses the scene queue, which aliases compute here.
        // Exercise a real dependency across both logical timelines on that queue.
        let source = gpu
            .create_buffer(
                16,
                vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST,
                false,
            )
            .unwrap();
        let output = gpu.create_readback_buffer(16).unwrap();
        gpu.begin_deferred_completion().unwrap();
        assert_eq!(
            gpu.native_queue(Queue::Graphics),
            gpu.native_queue(Queue::Compute)
        );
        let mut compute = gpu.begin(Queue::Compute);
        compute.push(Command::FillBuffer {
            destination: source,
            offset: 0,
            size: 16,
            value: 0x87654321,
        });
        compute.barriers([Barrier {
            resource: Resource::Buffer(source),
            before: Access::CopyDestination,
            after: Access::CopySource,
            handoff: None,
        }]);
        let produced = gpu.submit(compute.finish(), &[]).unwrap();
        let mut graphics = gpu.begin(Queue::Graphics);
        graphics.push(Command::CopyBuffer {
            source,
            destination: output,
            size: 16,
        });
        graphics.barriers([Barrier {
            resource: Resource::Buffer(output),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        gpu.submit(graphics.finish(), &[produced]).unwrap();
        let mut frame = gpu.end_deferred_completion().unwrap();
        gpu.wait_completion(&mut frame).unwrap();
        assert_eq!(
            gpu.read(output).unwrap(),
            0x87654321u32.to_le_bytes().repeat(4)
        );
        gpu.assert_clean().unwrap();
    }
    #[test]
    fn prefers_dedicated_compute_and_falls_back_to_the_graphics_family() {
        let mixed = vk::QueueFamilyProperties::default()
            .queue_flags(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
            .queue_count(1);
        let compute = vk::QueueFamilyProperties::default()
            .queue_flags(vk::QueueFlags::COMPUTE)
            .queue_count(1);
        assert_eq!(select(&[mixed]).unwrap(), (0, 0));
        assert_eq!(select(&[mixed, compute]).unwrap(), (0, 1));
        assert_eq!(select(&[mixed, mixed]).unwrap(), (0, 0));
        let absent = vk::QueueFamilyProperties::default()
            .queue_flags(vk::QueueFlags::COMPUTE)
            .queue_count(0);
        assert_eq!(select(&[mixed, absent]).unwrap(), (0, 0));
        assert!(select(&[mixed.queue_flags(vk::QueueFlags::GRAPHICS)]).is_err());
    }
}
