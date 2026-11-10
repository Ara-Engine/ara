#![allow(clippy::unwrap_used)]
//! A storage-image heap full of retired images (a resize's previous generation not
//! yet collected) returns their slots to the next allocation instead of failing.
use ara_gpu::{Device, Resource, vk, vulkan::VulkanDevice};
use std::path::Path;

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn retired_storage_images_free_their_slots_when_the_heap_is_full() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join("../../target/ara-gpu/storage-heap")).unwrap();
    let storage = |gpu: &mut VulkanDevice| {
        let image = gpu
            .create_image(
                8,
                8,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::STORAGE,
            )
            .unwrap();
        gpu.storage_image(image).map(|_| image)
    };
    let free = 128 - gpu.descriptor_usage().storage_images;
    let generation = (0..free)
        .map(|_| storage(&mut gpu).unwrap())
        .collect::<Vec<_>>();
    for image in generation {
        gpu.retire(Resource::Image(image)).unwrap();
    }
    // No collect ran since: every slot still belongs to a retired image.
    let next = storage(&mut gpu).unwrap();
    assert_eq!(gpu.descriptor_usage().storage_images, 128 - free + 1);
    gpu.retire(Resource::Image(next)).unwrap();
    gpu.collect().unwrap();
    gpu.assert_clean().unwrap();
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Vulkan GPU test; macOS runs the Metal backend"
)]
fn retired_storage_views_free_slots_for_new_views() {
    use ara_gpu::texture::{ImageViewDesc, ViewDimension};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu =
        VulkanDevice::new_raster(&[], root.join("../../target/ara-gpu/storage-views")).unwrap();
    let storage = |gpu: &mut VulkanDevice| {
        let image = gpu
            .create_mip_image(
                [8, 8],
                vk::Format::R16G16B16A16_SFLOAT,
                2,
                vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED,
            )
            .unwrap();
        gpu.storage_view(
            image,
            ImageViewDesc {
                dimension: ViewDimension::D2,
                base_mip: 1,
                mip_count: 1,
                base_layer: 0,
                layer_count: 1,
            },
        )
        .map(|_| image)
    };
    let baseline = gpu.descriptor_usage().storage_images;
    let generation = (0..128 - baseline)
        .map(|_| storage(&mut gpu).unwrap())
        .collect::<Vec<_>>();
    for image in generation {
        gpu.retire(Resource::Image(image)).unwrap();
    }
    // Reclaim through storage_view itself, without an explicit collection call.
    let next = storage(&mut gpu).unwrap();
    assert_eq!(gpu.descriptor_usage().storage_images, baseline + 1);
    gpu.retire(Resource::Image(next)).unwrap();
    gpu.collect().unwrap();
    gpu.assert_clean().unwrap();
}
