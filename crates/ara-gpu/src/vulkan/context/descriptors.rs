//! Joint heap sizing preserves the old minimum on adapters with tight totals.
use anyhow::{Result, ensure};
use ash::vk;

pub(super) struct Capacities {
    pub sampled_images: u32,
    pub storage_buffers: u32,
}

pub(super) fn select(
    limits: &vk::PhysicalDeviceDescriptorIndexingProperties<'_>,
    rt: bool,
) -> Result<Capacities> {
    // Reserve the legacy 128-image alias, at least 128 runtime images, 64 storage
    // images, three samplers and the optional AS binding before growing buffers.
    let reserved = 128 + 128 + 64 + 3 + if rt { 16 } else { 0 };
    let storage_buffers = [
        128,
        limits.max_per_stage_descriptor_update_after_bind_storage_buffers,
        limits.max_descriptor_set_update_after_bind_storage_buffers,
        limits
            .max_per_stage_update_after_bind_resources
            .saturating_sub(reserved),
        limits
            .max_update_after_bind_descriptors_in_all_pools
            .saturating_sub(reserved),
    ]
    .into_iter()
    .min()
    .unwrap_or(0);
    ensure!(
        storage_buffers >= 64,
        "Insufficient update-after-bind storage-buffer capacity"
    );
    let fixed = reserved - 128 + storage_buffers;
    let sampled_images = [
        65_536,
        limits
            .max_per_stage_descriptor_update_after_bind_sampled_images
            .saturating_sub(128),
        limits
            .max_descriptor_set_update_after_bind_sampled_images
            .saturating_sub(128),
        limits
            .max_per_stage_update_after_bind_resources
            .saturating_sub(fixed),
        limits
            .max_update_after_bind_descriptors_in_all_pools
            .saturating_sub(fixed),
    ]
    .into_iter()
    .min()
    .unwrap_or(0);
    ensure!(
        sampled_images >= 128,
        "Insufficient update-after-bind sampled-image capacity"
    );
    Ok(Capacities {
        sampled_images,
        storage_buffers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_growth_respects_both_buffer_and_shared_limits_without_losing_the_old_minimum()
    -> Result<()> {
        let mut limits = vk::PhysicalDeviceDescriptorIndexingProperties::default()
            .max_per_stage_descriptor_update_after_bind_storage_buffers(1024)
            .max_descriptor_set_update_after_bind_storage_buffers(1024)
            .max_per_stage_descriptor_update_after_bind_sampled_images(100_000)
            .max_descriptor_set_update_after_bind_sampled_images(100_000)
            .max_per_stage_update_after_bind_resources(100_000)
            .max_update_after_bind_descriptors_in_all_pools(100_000);
        let full = select(&limits, true)?;
        assert_eq!((full.storage_buffers, full.sampled_images), (128, 65_536));
        limits.max_per_stage_update_after_bind_resources = 400;
        let tight = select(&limits, false)?;
        assert_eq!((tight.storage_buffers, tight.sampled_images), (77, 128));
        limits.max_per_stage_update_after_bind_resources = 387;
        let old = select(&limits, false)?;
        assert_eq!((old.storage_buffers, old.sampled_images), (64, 128));
        assert!(select(&limits, true).is_err());
        limits.max_per_stage_update_after_bind_resources = 100_000;
        limits.max_descriptor_set_update_after_bind_storage_buffers = 63;
        assert!(select(&limits, false).is_err());
        Ok(())
    }
}
