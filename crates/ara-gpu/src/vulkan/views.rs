//! Shader-visible subresources with parent-image retirement.
use super::*;
use crate::texture::{ImageViewDesc, ViewDimension};
pub(super) struct View {
    parent: vk::Image,
    desc: ImageViewDesc,
    storage: bool,
    slot: HeapIndex,
    pub native: vk::ImageView,
}
impl VulkanDevice {
    /// Publish selected mips/layers as sampled. Declare the parent image in the graph.
    pub fn sampled_view(&mut self, image: Image, desc: ImageViewDesc) -> Result<HeapIndex> {
        self.texture_view(image, desc, false)
    }
    /// Publish one mip and selected layers as writable storage. Declare the parent image in the graph.
    pub fn storage_view(&mut self, image: Image, desc: ImageViewDesc) -> Result<HeapIndex> {
        self.texture_view(image, desc, true)
    }
    fn texture_view(
        &mut self,
        image: Image,
        desc: ImageViewDesc,
        storage: bool,
    ) -> Result<HeapIndex> {
        let raw = self.images.get(image)?.raw;
        desc.validate(raw.shape, storage)?;
        if !raw.usage.contains(if storage {
            vk::ImageUsageFlags::STORAGE
        } else {
            vk::ImageUsageFlags::SAMPLED
        }) {
            return Err(GpuError::Contract(
                "Texture view usage was not declared".into(),
            ));
        }
        if let Some(view) = self
            .views
            .iter()
            .find(|v| v.parent == raw.handle && v.desc == desc && v.storage == storage)
        {
            return Ok(view.slot);
        }
        let kind = match desc.dimension {
            ViewDimension::D2 => vk::ImageViewType::TYPE_2D,
            ViewDimension::D2Array => vk::ImageViewType::TYPE_2D_ARRAY,
            ViewDimension::D3 => vk::ImageViewType::TYPE_3D,
        };
        let format = if storage {
            context::allocation::storage_alias(raw.format).unwrap_or(raw.format)
        } else {
            raw.format
        };
        let mut usage = vk::ImageViewUsageCreateInfo::default().usage(if storage {
            vk::ImageUsageFlags::STORAGE
        } else {
            vk::ImageUsageFlags::SAMPLED
        });
        // SAFETY: checked dimensions/ranges; native parent allocation outlives this view.
        let native = unsafe {
            self.context.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(raw.handle)
                    .view_type(kind)
                    .format(format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(raw.aspect)
                            .base_mip_level(desc.base_mip)
                            .level_count(desc.mip_count)
                            .base_array_layer(desc.base_layer)
                            .layer_count(desc.layer_count),
                    )
                    .push(&mut usage),
                None,
            )
        }
        .map_err(anyhow::Error::from)?;
        let slot = self.heap.view(&self.context, native, storage);
        let slot = match slot {
            Ok(slot) => slot,
            Err(e) => {
                // SAFETY: view publication failed; it has no GPU references.
                unsafe {
                    self.context.device.destroy_image_view(native, None);
                }
                return Err(e);
            }
        };
        self.views.push(View {
            parent: raw.handle,
            desc,
            storage,
            slot,
            native,
        });
        Ok(slot)
    }
    pub(super) fn retire_views(&mut self, parent: vk::Image) {
        let mut i = 0;
        while i < self.views.len() {
            if self.views[i].parent != parent {
                i += 1;
                continue;
            }
            let view = self.views.swap_remove(i);
            self.heap.retire_view(view.slot, view.storage);
            // SAFETY: the parent image's references completed on both timelines.
            unsafe {
                self.context.device.destroy_image_view(view.native, None);
            }
        }
    }
}
