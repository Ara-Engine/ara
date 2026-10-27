//! CAMetalLayer presentation, the Metal counterpart of `ara-platform`'s Vulkan
//! `Present`. The layer is BGRA8 with sRGB-encoded values (what the Vulkan path
//! selects as `B8G8R8A8_UNORM` + `SRGB_NONLINEAR`), so engine output written in the
//! reported surface format is presented by a plain copy into the drawable.
//!
//! Like the Vulkan v0 presenter, `display` is serialized: it orders the copy after all
//! submitted work, presents, and waits for its own submission before returning.
//! Scaled presentation and HDR (EDR) output are not implemented yet.
use super::MetalDevice;
use super::native::{nil, pool};
use crate::{Device, GpuError, Image, Queue, Result, TimelinePoint};
use ash::vk;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_metal::*;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use std::ptr::NonNull;

/// The presented pixel format and its portable description.
const PIXEL: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
const SURFACE: vk::SurfaceFormatKHR = vk::SurfaceFormatKHR {
    format: vk::Format::B8G8R8A8_UNORM,
    color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
};

/// A CAMetalLayer bound to one device, presenting completed engine images.
pub struct Presenter {
    layer: Retained<CAMetalLayer>,
    /// The graphics queue holding the layer's residency set.
    queue: Retained<ProtocolObject<dyn MTL4CommandQueue>>,
    size: [u32; 2],
}

impl Presenter {
    /// A windowless layer, for offscreen hosts and tests.
    pub fn new(gpu: &MetalDevice, size: [u32; 2]) -> Result<Self> {
        Self::configure(gpu, CAMetalLayer::new(), size)
    }
    /// Present into an existing layer, e.g. from `SDL_Metal_GetLayer`.
    /// # Safety
    /// `layer` must point to a live `CAMetalLayer` that outlives this presenter and is
    /// presented only through it.
    pub unsafe fn from_layer(
        gpu: &MetalDevice,
        layer: NonNull<std::ffi::c_void>,
        size: [u32; 2],
    ) -> Result<Self> {
        // SAFETY: the caller guarantees a live CAMetalLayer; retaining it keeps it alive.
        let layer = unsafe { Retained::retain(layer.cast::<CAMetalLayer>().as_ptr()) }
            .ok_or_else(|| nil("CAMetalLayer"))?;
        Self::configure(gpu, layer, size)
    }
    fn configure(gpu: &MetalDevice, layer: Retained<CAMetalLayer>, size: [u32; 2]) -> Result<Self> {
        pool(|| Self::configure_now(gpu, layer, size))
    }
    fn configure_now(
        gpu: &MetalDevice,
        layer: Retained<CAMetalLayer>,
        size: [u32; 2],
    ) -> Result<Self> {
        layer.setDevice(Some(&gpu.device));
        layer.setPixelFormat(PIXEL);
        // Copies write the drawable, so it cannot be a framebuffer-only texture.
        layer.setFramebufferOnly(false);
        // Drawable textures live in the layer's residency set (macOS 15+).
        let queue = gpu.queues[Queue::Graphics.index()].clone();
        queue.addResidencySet(&layer.residencySet());
        let mut presenter = Self {
            layer,
            queue,
            size: [0; 2],
        };
        presenter.resize(size)?;
        Ok(presenter)
    }
    /// Match a new pixel extent; the next `display` uses drawables of this size.
    pub fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        if size.contains(&0) {
            return Err(GpuError::Contract("Empty presentation extent".into()));
        }
        self.layer.setDrawableSize(CGSize {
            width: f64::from(size[0]),
            height: f64::from(size[1]),
        });
        self.size = size;
        Ok(())
    }
    /// The format engine output must use to be presented by copy.
    pub fn surface_format(&self) -> vk::SurfaceFormatKHR {
        SURFACE
    }
    /// Current drawable extent.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    /// Present a `B8G8R8A8_UNORM` image of the presenter's size with transfer-source
    /// usage, after all work already submitted on this device.
    pub fn display(&mut self, gpu: &mut MetalDevice, source: Image) -> Result<()> {
        self.present(gpu, source, None)
    }

    /// `readback` (tests) also copies the drawable into a host buffer before presenting.
    pub(super) fn present(
        &mut self,
        gpu: &mut MetalDevice,
        source: Image,
        readback: Option<crate::Buffer>,
    ) -> Result<()> {
        pool(|| self.present_now(gpu, source, readback))
    }
    fn present_now(
        &mut self,
        gpu: &mut MetalDevice,
        source: Image,
        readback: Option<crate::Buffer>,
    ) -> Result<()> {
        let image = &gpu.images.get(source)?.raw;
        if image.size != self.size
            || image.info.pixel != PIXEL
            || !image.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
        {
            return Err(GpuError::Contract(format!(
                "Presented images must be {}x{} B8G8R8A8_UNORM transfer sources",
                self.size[0], self.size[1]
            )));
        }
        let texture = image.texture.clone();
        let readback = readback
            .map(|b| gpu.buffers.get(b).map(|b| b.raw.buffer.clone()))
            .transpose()?;
        let drawable = self
            .layer
            .nextDrawable()
            .ok_or_else(|| nil("CAMetalLayer nextDrawable"))?;
        let target = drawable.texture();
        let queue = Queue::Graphics;
        let point = TimelinePoint {
            queue,
            value: gpu.submitted[queue.index()] + 1,
        };
        let allocator = match gpu.allocators.pop() {
            Some(allocator) => allocator,
            None => gpu
                .device
                .newCommandAllocator()
                .ok_or_else(|| nil("command allocator"))?,
        };
        let cmd = gpu
            .device
            .newCommandBuffer()
            .ok_or_else(|| nil("command buffer"))?;
        cmd.beginCommandBufferWithAllocator(&allocator);
        let encoder = cmd
            .computeCommandEncoder()
            .ok_or_else(|| nil("compute encoder"))?;
        // Every earlier write to the source on this queue precedes the copy.
        encoder.barrierAfterQueueStages_beforeStages_visibilityOptions(
            MTLStages::All,
            MTLStages::Blit,
            MTL4VisibilityOptions::Device,
        );
        let [width, height] = self.size;
        let extent = MTLSize {
            width: width as usize,
            height: height as usize,
            depth: 1,
        };
        let origin = MTLOrigin { x: 0, y: 0, z: 0 };
        // SAFETY: equally sized BGRA8 textures (checked); the drawable is resident
        // through the layer's residency set and the source through the device's.
        unsafe {
            encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                &texture, 0, 0, origin, extent, &target, 0, 0, origin,
            );
        }
        if let Some(buffer) = &readback {
            encoder.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                MTLStages::Blit,
                MTLStages::Blit,
                MTL4VisibilityOptions::Device,
            );
            let row = width as usize * 4;
            // SAFETY: the readback buffer was created by this device; callers size it
            // for the whole drawable (tests only).
            unsafe {
                encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                    &target, 0, 0, origin, extent, buffer, 0, row, row * height as usize,
                );
            }
        }
        encoder.endEncoding();
        cmd.endCommandBuffer();
        if std::mem::take(&mut gpu.residency_dirty) {
            gpu.residency.commit();
        }
        let native = &gpu.queues[queue.index()];
        // The source may have been written on the compute queue too.
        for other in [Queue::Graphics, Queue::Compute] {
            let value = gpu.submitted[other.index()];
            if value > 0 {
                native.waitForEvent_value(
                    ProtocolObject::from_ref(&*gpu.events[other.index()]),
                    value,
                );
            }
        }
        let mtl_drawable = ProtocolObject::from_ref(&*drawable);
        native.waitForDrawable(mtl_drawable);
        let buffers = [NonNull::from(&*cmd)];
        // SAFETY: one ended, never-committed command buffer; the array outlives the call.
        unsafe { native.commit_count(NonNull::from(&buffers[0]), 1) };
        native.signalDrawable(mtl_drawable);
        native.signalEvent_value(
            ProtocolObject::from_ref(&*gpu.events[queue.index()]),
            point.value,
        );
        drawable.present();
        gpu.submitted[queue.index()] = point.value;
        gpu.images.get_mut(source)?.last[queue.index()] = point.value;
        gpu.pending.push(super::Pending {
            point,
            allocator,
            chunks: Vec::new(),
            timing: None,
        });
        Device::wait(gpu, point)
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        // `display` waited for its own submission, so no drawable is still in use.
        self.queue.removeResidencySet(&self.layer.residencySet());
    }
}
