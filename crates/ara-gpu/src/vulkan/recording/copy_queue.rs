//! Queue-capability preflight for whole and regional buffer/image transfers.
use crate::{Image, Result, commands::Command, vk};
#[cfg(test)]
thread_local! {static FORCE_COMPUTE_ONLY:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
// A test-only capability override makes the rejection path reproducible on a
// universal-family Radeon without submitting an illegal native copy.
pub(super) fn capabilities(_queue: crate::Queue, flags: vk::QueueFlags) -> vk::QueueFlags {
    #[cfg(test)]
    if _queue == crate::Queue::Compute && FORCE_COMPUTE_ONLY.get() {
        return flags & !vk::QueueFlags::GRAPHICS;
    }
    flags
}
pub(super) fn validate(
    command: &Command,
    flags: vk::QueueFlags,
    mut aspect: impl FnMut(Image) -> Result<vk::ImageAspectFlags>,
) -> Result<()> {
    let image = match command {
        Command::UploadImage { destination, .. }
        | Command::UploadImageRegion { destination, .. } => *destination,
        Command::ReadImage { source, .. } | Command::ReadImageRegion { source, .. } => *source,
        _ => return Ok(()),
    };
    let aspect = aspect(image)?;
    // maintenance10 is not enabled. This applies to both copy directions and
    // to whole-image commands as well as regional copies (VUIDs 07739/10216).
    if aspect.intersects(vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL)
        && !flags.contains(vk::QueueFlags::GRAPHICS)
    {
        return Err(crate::GpuError::Contract("Depth/stencil buffer-image copies require a graphics-capable queue family; use Queue::Graphics".into()));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BufferKind, ImageKind, arena::Arena, texture::ImageRegion};
    pub(super) fn copies(image: Image, buffer: crate::Buffer) -> [Command; 4] {
        let region = ImageRegion {
            mip: 0,
            base_layer: 0,
            layer_count: 1,
            origin: [0; 3],
            extent: [2, 2, 1],
        };
        [
            Command::UploadImage {
                source: buffer,
                destination: image,
            },
            Command::ReadImage {
                source: image,
                destination: buffer,
            },
            Command::UploadImageRegion {
                source: buffer,
                destination: image,
                buffer_offset: 0,
                region,
            },
            Command::ReadImageRegion {
                source: image,
                destination: buffer,
                buffer_offset: 0,
                region,
            },
        ]
    }
    fn fake_buffer() -> crate::Buffer {
        Arena::<BufferKind, ()>::default().insert(()).unwrap()
    }
    #[test]
    fn regression_compute_only_rejects_all_depth_buffer_image_copies() {
        let mut admitted = Vec::new();
        for format in [vk::Format::D16_UNORM, vk::Format::D32_SFLOAT] {
            let mut images: Arena<ImageKind, vk::Format> = Arena::default();
            let image = images.insert(format).unwrap();
            for (index, command) in copies(image, fake_buffer()).into_iter().enumerate() {
                let result = validate(
                    &command,
                    vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
                    |i| {
                        assert_eq!(*images.get(i)?, format);
                        Ok(vk::ImageAspectFlags::DEPTH)
                    },
                );
                if !matches!(result, Err(crate::GpuError::Contract(_))) {
                    admitted.push((format, index));
                }
            }
        }
        assert!(
            admitted.is_empty(),
            "compute-only family admitted depth-copy variants {admitted:?}"
        );
    }
    #[test]
    fn shared_graphics_compute_family_allows_depth_buffer_image_copies() {
        let mut images: Arena<ImageKind, ()> = Arena::default();
        let image = images.insert(()).unwrap();
        for command in copies(image, fake_buffer()) {
            validate(
                &command,
                vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE,
                |_| Ok(vk::ImageAspectFlags::DEPTH),
            )
            .unwrap();
        }
    }
    #[test]
    fn compute_only_family_allows_color_buffer_image_copies() {
        let mut images: Arena<ImageKind, ()> = Arena::default();
        let image = images.insert(()).unwrap();
        for command in copies(image, fake_buffer()) {
            validate(
                &command,
                vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
                |_| Ok(vk::ImageAspectFlags::COLOR),
            )
            .unwrap();
        }
    }
}

#[cfg(test)]
mod hardware {
    use super::*;
    use crate::{
        Device, Queue, Resource,
        descriptors::{Format, ImageDesc},
        vulkan::VulkanDevice,
    };
    #[test]
    #[cfg_attr(target_os = "macos", ignore = "Vulkan hardware probe")]
    fn forced_compute_only_depth_copies_fail_before_native_submission() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                FORCE_COMPUTE_ONLY.set(false);
            }
        }
        let mut gpu =
            VulkanDevice::new_raster(&[], std::env::temp_dir().join("ara-depth-copy-queue"))
                .unwrap();
        FORCE_COMPUTE_ONLY.set(true);
        let _reset = Reset;
        let buffer = gpu
            .create_buffer(
                32,
                vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST,
                true,
            )
            .unwrap();
        gpu.upload(buffer, &[0; 32]).unwrap();
        let submitted = gpu.submitted;
        for format in [Format::Depth16Unorm, Format::Depth32Float] {
            let image = Device::create_image(
                &mut gpu,
                &ImageDesc {
                    size: [2, 2],
                    format,
                    sampled: false,
                    storage: false,
                    attachment: false,
                    copy_source: true,
                    copy_destination: true,
                },
            )
            .unwrap();
            for command in super::tests::copies(image, buffer) {
                let mut rec = gpu.begin(Queue::Compute);
                rec.push(command);
                let error = gpu.submit(rec.finish(), &[]).unwrap_err();
                assert!(
                    error.to_string().contains("graphics-capable queue family"),
                    "{error}"
                );
                assert_eq!(
                    gpu.submitted, submitted,
                    "rejected depth copy mutated submission state"
                );
            }
            gpu.retire(Resource::Image(image)).unwrap();
        }
        gpu.retire(Resource::Buffer(buffer)).unwrap();
        gpu.collect().unwrap();
        gpu.assert_clean().unwrap();
    }
}
