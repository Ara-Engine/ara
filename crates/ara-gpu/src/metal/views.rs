//! Immutable subresource views retained and retired with their parent texture.
use super::*;
use crate::texture::{ImageViewDesc, ViewDimension};
pub(super) struct View {
    parent: u64,
    desc: ImageViewDesc,
    storage: bool,
    slot: HeapIndex,
    _texture: Retained<ProtocolObject<dyn MTLTexture>>,
}
impl MetalDevice {
    /// Publish selected mips/layers as a sampled texture. Declare the parent image in the graph.
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
        native::pool(|| {
            let raw = &self.images.get(image)?.raw;
            desc.validate(raw.shape, raw.size, storage)?;
            if !raw.usage.contains(if storage {
                vk::ImageUsageFlags::STORAGE
            } else {
                vk::ImageUsageFlags::SAMPLED
            }) {
                return Err(GpuError::Contract(
                    "Texture view usage was not declared".into(),
                ));
            }
            let parent = raw.texture.gpuResourceID().to_raw();
            if let Some(view) = self
                .views
                .iter()
                .find(|v| v.parent == parent && v.desc == desc && v.storage == storage)
            {
                return Ok(view.slot);
            }
            let kind = match desc.dimension {
                ViewDimension::D2 => MTLTextureType::Type2D,
                ViewDimension::D2Array => MTLTextureType::Type2DArray,
                ViewDimension::D3 => MTLTextureType::Type3D,
                ViewDimension::CubeArray => MTLTextureType::TypeCubeArray,
            };
            // SAFETY: dimension and mip/layer ranges were checked against the parent allocation.
            let texture = unsafe {
                raw.texture
                    .newTextureViewWithPixelFormat_textureType_levels_slices(
                        raw.info.pixel,
                        kind,
                        objc2_foundation::NSRange::new(
                            desc.base_mip as usize,
                            desc.mip_count as usize,
                        ),
                        objc2_foundation::NSRange::new(
                            desc.base_layer as usize,
                            desc.layer_count as usize,
                        ),
                    )
            }
            .ok_or_else(|| nil("texture view"))?;
            let entry = abi::TableEntry::texture(texture.gpuResourceID().to_raw());
            let slot = if storage {
                self.tables.storage_images.insert(entry)?
            } else {
                self.tables.textures.insert(entry)?
            };
            self.residency
                .addAllocation(ProtocolObject::from_ref(&*texture));
            self.residency_dirty = true;
            self.views.push(View {
                parent,
                desc,
                storage,
                slot,
                _texture: texture,
            });
            Ok(slot)
        })
    }
    pub(super) fn retire_views(&mut self, parent: u64) {
        let mut i = 0;
        while i < self.views.len() {
            if self.views[i].parent != parent {
                i += 1;
                continue;
            }
            let view = self.views.swap_remove(i);
            self.residency
                .removeAllocation(ProtocolObject::from_ref(&*view._texture));
            self.residency_dirty = true;
            if view.storage {
                self.tables.storage_images.release(view.slot);
            } else {
                self.tables.textures.release(view.slot);
            }
        }
    }
}
