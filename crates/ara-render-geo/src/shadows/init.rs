use super::*;
impl VirtualShadows {
    /// Allocates a fixed depth page pool and prewarms all offline shadow passes.
    /// Call during scene preparation, not on the interactive render thread.
    pub fn new(
        gpu: &mut VulkanDevice,
        geometry: &GeometryView,
        page_capacity: u32,
        cluster_capacity: u32,
        cooked: &Path,
    ) -> Result<Self> {
        ensure!(
            (1..=16384).contains(&page_capacity) && cluster_capacity > 0,
            "invalid VSM pool capacities"
        );
        let virtual_pages = 6 * 128 * 128;
        let mut buffers = Vec::new();
        for (i, size) in [
            virtual_pages * 4,
            virtual_pages * 4,
            page_capacity as usize * 16,
            page_capacity as usize * 128 * 128 * 4,
            128,
            page_capacity as usize * 4,
            cluster_capacity as usize * 16,
            128,
        ]
        .into_iter()
        .enumerate()
        {
            if i == 3 {
                // Only assigned dirty pages are ever sampled, and shadow_clear
                // initializes those pages before rasterization. Avoid a gigabyte
                // of unnecessary CPU zeroes and staging traffic.
                buffers.push(gpu.create_buffer(
                    size as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    false,
                )?);
                continue;
            }
            buffers.push(crate::init::upload(
                gpu,
                &vec![0; size],
                if i == 4 || i == 7 {
                    vk::BufferUsageFlags::INDIRECT_BUFFER
                } else {
                    vk::BufferUsageFlags::empty()
                },
            )?);
        }
        let buffers: [Buffer; 8] = buffers
            .try_into()
            .map_err(|_| anyhow::anyhow!("VSM buffer count"))?;
        let mut slots = [0; 8];
        for (i, &b) in buffers.iter().enumerate() {
            slots[i] = gpu.storage_buffer(b)?.index;
        }
        let frame = gpu.create_buffer(
            size_of::<ShadowFrame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let mask = gpu.create_image(
            geometry.size[0],
            geometry.size[1],
            vk::Format::R32G32B32A32_SFLOAT,
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let mask_slot = gpu.storage_image(mask)?.index;
        let world_slot = gpu.sampled_image(geometry.images[2], false)?.index;
        let normal_slot = gpu.sampled_image(geometry.images[1], false)?.index;
        let preview_slot = geometry.image_slots[3];
        let mut pipelines = BTreeMap::new();
        for name in [
            "shadow_reset",
            "shadow_mark",
            "shadow_allocate",
            "shadow_finalize",
            "shadow_clear",
            "shadow_select",
            "shadow_arguments",
            "shadow_raster",
            "shadow_resolve",
        ] {
            let p: CookedPipeline =
                serde_json::from_slice(&std::fs::read(cooked.join(format!("{name}.json")))?)?;
            pipelines.insert(name.into(), gpu.create_pipeline(&p, cooked)?);
        }
        Ok(Self {
            frame,
            buffers,
            slots,
            pipelines,
            mask,
            mask_slot,
            world_slot,
            normal_slot,
            preview_slot,
            previous: Vec::new(),
            size: geometry.size,
            capacity: page_capacity,
            cluster_capacity,
            frame_id: 0,
            matrices: [Mat4::ZERO; 6],
            revisions: [0; 6],
            projection_count: 0,
            geometry_revision: 0,
            last: None,
        })
    }
}
