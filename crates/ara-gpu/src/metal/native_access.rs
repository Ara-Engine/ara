//! Native texture access for same-GPU integrations (the Metal editor UI), the
//! counterpart of the Vulkan backend's `native_image`. Every `MetalDevice` wraps the
//! same system `MTLDevice`, so one device's texture can be sampled by another once
//! adopted; adoption retains the texture, which stays alive while the borrower holds
//! it, so the owner may retire its own handle at any time.
use super::MetalDevice;
use super::native::{engine_format, format};
use super::resources::NativeImage;
use crate::{GpuError, Image, Result};
use ash::vk;
use objc2_metal::*;

impl MetalDevice {
    /// The native texture of `image` ([`crate::NativeImage`]), retained.
    pub fn native_image(&self, image: Image) -> Result<crate::NativeImage> {
        Ok(self.images.get(image)?.raw.texture.clone())
    }
    /// Wrap a single-level texture of the same `MTLDevice` (another device's image, an
    /// IOSurface slot) as an image here: published to this device's tables, copied and
    /// retired like any other image. `usage` describes it as created; its engine format
    /// follows from its pixel format.
    pub fn adopt_texture(
        &mut self,
        texture: crate::NativeImage,
        usage: vk::ImageUsageFlags,
    ) -> Result<Image> {
        let pixel = texture.pixelFormat();
        let engine = engine_format(pixel).ok_or_else(|| {
            GpuError::Contract(format!(
                "Adopted texture format {pixel:?} has no engine format"
            ))
        })?;
        if texture.mipmapLevelCount() != 1 {
            return Err(GpuError::Contract(
                "Adopted textures have a single mip level".into(),
            ));
        }
        let info = format(engine)?;
        if texture.device().registryID() != self.device.registryID() {
            return Err(GpuError::Contract(
                "Adopted texture belongs to another GPU".into(),
            ));
        }
        let size = [texture.width() as u32, texture.height() as u32];
        self.adopt_image(NativeImage {
            texture,
            info,
            size,
            levels: 1,
            usage,
            storage: None,
        })
    }
}
