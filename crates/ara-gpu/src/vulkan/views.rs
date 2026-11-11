//! Shader-visible subresources with parent-image retirement.
use super::*;
use crate::texture::{ImageViewDesc, ViewDimension};
pub(super) struct View {
    parent: vk::Image,
    desc: ImageViewDesc,
    storage: bool,
    slot: Option<HeapIndex>,
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
        if let Some(slot) = self.views.iter().find_map(|v| {
            (v.parent == raw.handle && v.desc == desc && v.storage == storage)
                .then_some(v.slot)
                .flatten()
        }) {
            return Ok(slot);
        }
        if storage && self.heap.storage_full() {
            self.reclaim_storage()?;
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
            slot: Some(slot),
            native,
        });
        Ok(slot)
    }
    pub(super) fn prepare_render_views(&mut self, list: &CommandList) -> Result<()> {
        for command in &list.commands {
            if let Command::BeginRenderingSubresources {
                colors,
                depth,
                resolves,
                ..
            } = command
            {
                for target in colors
                    .iter()
                    .map(|a| a.target)
                    .chain(depth.iter().map(|a| a.target))
                    .chain(resolves.iter().flatten().copied())
                {
                    let raw = self.images.get(target.image)?.raw;
                    if whole_view_is_the_slice(target, raw.shape) {
                        continue;
                    }
                    let desc = attachment_desc(target);
                    if self
                        .views
                        .iter()
                        .any(|v| v.parent == raw.handle && v.desc == desc && v.slot.is_none())
                    {
                        continue;
                    }
                    let mut usage = vk::ImageViewUsageCreateInfo::default().usage(
                        if raw.aspect == vk::ImageAspectFlags::DEPTH {
                            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                        } else {
                            vk::ImageUsageFlags::COLOR_ATTACHMENT
                        },
                    );
                    // SAFETY: command validation checked usage and the single mip/layer; the live parent owns this cached view.
                    let native = unsafe {
                        self.context.device.create_image_view(
                            &vk::ImageViewCreateInfo::default()
                                .image(raw.handle)
                                .view_type(vk::ImageViewType::TYPE_2D)
                                .format(raw.format)
                                .subresource_range(
                                    vk::ImageSubresourceRange::default()
                                        .aspect_mask(raw.aspect)
                                        .base_mip_level(target.mip)
                                        .level_count(1)
                                        .base_array_layer(target.layer)
                                        .layer_count(1),
                                )
                                .push(&mut usage),
                            None,
                        )
                    }
                    .map_err(anyhow::Error::from)?;
                    self.views.push(View {
                        parent: raw.handle,
                        desc,
                        storage: false,
                        slot: None,
                        native,
                    });
                }
            }
        }
        Ok(())
    }
    pub(super) fn render_view(
        &self,
        target: crate::rendering::RenderTarget,
    ) -> Result<vk::ImageView> {
        let raw = self.images.get(target.image)?.raw;
        if whole_view_is_the_slice(target, raw.shape) {
            return Ok(raw.attachment_view.unwrap_or(raw.view));
        }
        let desc = attachment_desc(target);
        self.views
            .iter()
            .find(|v| v.parent == raw.handle && v.desc == desc && v.slot.is_none())
            .map(|v| v.native)
            .ok_or_else(|| GpuError::Contract("Render target view was not prepared".into()))
    }
    pub(super) fn has_storage_views(&self, parent: vk::Image) -> bool {
        self.views.iter().any(|v| v.parent == parent && v.storage)
    }
    pub(super) fn retire_views(&mut self, parent: vk::Image) {
        let mut i = 0;
        while i < self.views.len() {
            if self.views[i].parent != parent {
                i += 1;
                continue;
            }
            let view = self.views.swap_remove(i);
            if let Some(slot) = view.slot {
                self.heap.retire_view(slot, view.storage);
            }
            // SAFETY: the parent image's references completed on both timelines.
            unsafe {
                self.context.device.destroy_image_view(view.native, None);
            }
        }
    }
}

/// The image's own attachment view serves a selected target only when it is exactly
/// that slice: mip 0 of an image with one layer. On an array it spans every layer, so
/// attaching it for layer 0 would also require (and expose) the sibling layers.
fn whole_view_is_the_slice(
    target: crate::rendering::RenderTarget,
    shape: crate::texture::ImageShape,
) -> bool {
    target.mip == 0 && target.layer == 0 && shape.layers == 1
}
fn attachment_desc(target: crate::rendering::RenderTarget) -> ImageViewDesc {
    ImageViewDesc {
        dimension: ViewDimension::D2,
        base_mip: target.mip,
        mip_count: 1,
        base_layer: target.layer,
        layer_count: 1,
    }
}
