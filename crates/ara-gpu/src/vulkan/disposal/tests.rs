use super::*;
use crate::{Device, Resource, vulkan::VulkanDevice};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

struct Release(Option<Sender<()>>);

impl Release {
    fn open(&mut self) {
        if let Some(send) = self.0.take() {
            let _ = send.send(());
        }
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn completed_images_release_heap_slots_while_driver_destruction_is_blocked() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-gpu/async-disposal");
    let mut gpu = VulkanDevice::new_raster(&[], root)?;
    let (send, receive) = channel();
    let mut release = Release(Some(send));
    let (entered, started) = channel();
    let mut gate: Option<Receiver<()>> = Some(receive);
    let disposal = Disposal::start(gpu.context.device.clone(), move || {
        if let Some(gate) = gate.take() {
            let _ = entered.send(());
            let _ = gate.recv();
        }
    })?;
    drop(gpu.disposal.replace(disposal));
    let baseline = gpu.descriptor_usage();
    let usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
    let image = gpu.create_image(64, 64, vk::Format::R8G8B8A8_UNORM, usage)?;
    let raw = gpu.native_image(image)?;
    let first_index = gpu.sampled_image(image, false)?.index;
    gpu.retire(Resource::Image(image))?;
    // Larger than the idle-memory pool, so this completed block must be freed
    // by the worker too; small reusable buffer allocations stay on the owner.
    let large = gpu.create_buffer(65 * 1024 * 1024, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.retire(Resource::Buffer(large))?;
    let waits = gpu.context.host_wait_stats().calls;
    gpu.collect()?;
    started.recv_timeout(Duration::from_secs(5))?;
    assert_eq!(gpu.descriptor_usage(), baseline);
    assert!(
        gpu.native_image(image).is_err(),
        "CPU handle expires immediately"
    );
    // SAFETY: The worker is physically held before destruction. Its ownership
    // keeps the retired Vulkan image/allocation live, independently of the arena.
    let requirements = unsafe { gpu.context.device.get_image_memory_requirements(raw) };
    assert!(requirements.size > 0);
    // Saturate the single queued batch while the driver worker is held. Owner
    // collection and descriptor reuse must keep progressing without a host wait.
    for _ in 0..64 {
        let replacement = gpu.create_image(8, 8, vk::Format::R8G8B8A8_UNORM, usage)?;
        assert_eq!(gpu.sampled_image(replacement, false)?.index, first_index);
        assert_ne!(image, replacement);
        gpu.retire(Resource::Image(replacement))?;
        gpu.collect()?;
        assert_eq!(gpu.descriptor_usage(), baseline);
    }
    assert_eq!(gpu.context.host_wait_stats().calls, waits);
    release.open();
    // Joining drains active, queued and owner-retained batches before the Vulkan
    // device goes away; sync validation catches premature or duplicate release.
    drop(gpu.disposal.take());
    gpu.assert_clean()?;
    Ok(())
}
