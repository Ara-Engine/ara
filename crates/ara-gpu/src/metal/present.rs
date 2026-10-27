//! CAMetalLayer presentation, the Metal counterpart of `ara-platform`'s Vulkan
//! `Present`. The layer is BGRA8 with sRGB-encoded values (what the Vulkan path
//! selects as `B8G8R8A8_UNORM` + `SRGB_NONLINEAR`). Like the Vulkan blit, `display`
//! scales and converts any completed colour image to the drawable ([`super::transfer`]).
//!
//! Like the Vulkan presenter, `display` orders the copy after all submitted work and
//! presents; it waits for its own command buffer before returning unless frames are
//! in flight (`set_frames_in_flight`), when the next display waits for it instead.
//! HDR (EDR) output is not implemented yet.
use super::MetalDevice;
use super::native::{nil, pool};
use super::transfer::{CommandBuffer, Transfer};
use crate::{GpuError, Image, Result};
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
    transfer: Transfer,
    size: [u32; 2],
    /// `display` returns once the copy is committed (`set_frames_in_flight`).
    frames_in_flight: bool,
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
        // Blits and the scale kernel write the drawable, so it cannot be a
        // framebuffer-only texture.
        layer.setFramebufferOnly(false);
        let mut presenter = Self {
            layer,
            transfer: Transfer::new(gpu)?,
            size: [0; 2],
            frames_in_flight: false,
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
    /// The drawable format; engine output in any colour format is converted.
    pub fn surface_format(&self) -> vk::SurfaceFormatKHR {
        SURFACE
    }
    /// Let `display` return once its copy is committed, GPU-ordered after the
    /// device's submitted work; the next display waits for it first. With
    /// CAMetalLayer's drawable pool this keeps at most one present in flight, so the
    /// host overlaps the next frame's CPU work. Turning it off waits now.
    pub fn set_frames_in_flight(&mut self, enabled: bool) -> Result<()> {
        self.frames_in_flight = enabled;
        if enabled {
            Ok(())
        } else {
            self.transfer.wait_pending(None)
        }
    }
    /// Order `gpu`'s next commits after the copies presented so far (a frame in
    /// flight reads the image the next frame rewrites).
    pub fn order_before(&self, gpu: &mut MetalDevice) {
        self.transfer.order_before(gpu);
    }
    /// Current drawable extent.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    /// Present a completed colour image with transfer-source usage, scaled to the
    /// drawable, after all work already submitted on this device.
    pub fn display(&mut self, gpu: &MetalDevice, source: Image) -> Result<()> {
        self.present(gpu, source, None)
    }

    /// `readback` (tests) also copies the drawable into a host buffer before presenting.
    pub(super) fn present(
        &mut self,
        gpu: &MetalDevice,
        source: Image,
        readback: Option<crate::Buffer>,
    ) -> Result<()> {
        pool(|| self.present_now(gpu, source, readback))
    }
    fn present_now(
        &mut self,
        gpu: &MetalDevice,
        source: Image,
        readback: Option<crate::Buffer>,
    ) -> Result<()> {
        let image = &gpu.images.get(source)?.raw;
        if image.info.depth || !image.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC) {
            return Err(GpuError::Contract(
                "Presented images must be colour transfer sources".into(),
            ));
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
        let [width, height] = self.size.map(|v| v as usize);
        let frames_in_flight = self.frames_in_flight && readback.is_none();
        let record = |transfer: &Transfer, cmd: &CommandBuffer| {
            transfer.copy(cmd, &texture, &target)?;
            if let Some(buffer) = &readback {
                let blit = cmd
                    .blitCommandEncoder()
                    .ok_or_else(|| nil("blit encoder"))?;
                let extent = MTLSize {
                    width,
                    height,
                    depth: 1,
                };
                // SAFETY: callers size the readback buffer for the whole drawable
                // (tests only).
                unsafe {
                    blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                        &target, 0, 0, MTLOrigin { x: 0, y: 0, z: 0 }, extent,
                        buffer, 0, width * 4, width * height * 4,
                    );
                }
                blit.endEncoding();
            }
            cmd.presentDrawable(ProtocolObject::from_ref(&*drawable));
            Ok(())
        };
        if frames_in_flight {
            self.transfer.submit(gpu, record)
        } else {
            self.transfer.run(Some(gpu), record)
        }
    }
}
