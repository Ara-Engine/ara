//! Shared surfaces on Metal (D19 embedded Play): the [`crate::shared`] protocol over
//! IOSurfaces. The producer's ring slots are global IOSurfaces (their IDs travel as
//! [`SlotDesc::handle`]); the owner looks them up ([`SharedSurface`]) and samples
//! them as ordinary images. Slot textures live in each device's image arena, so
//! copies, residency and retirement use the normal submission path. Unified memory
//! needs no queue-family transfer: the producer waits for its copy before sending
//! `FrameReady`, and the owner releases a slot only after its own sampling completed.
use super::MetalDevice;
use super::native::{format, nil};
use super::resources::NativeImage;
use crate::commands::Command;
use crate::shared::{
    Frame, IoSurface, Ring, SLOTS, SharedSurface, SlotDesc, SurfaceDesc, SurfaceFormat,
};
use crate::{Device, GpuError, Image, Queue, Resource, Result};
use ash::vk;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFString, CFType};
// Global surfaces are the deliberate D19 transport here (see `shared::SharedSurface`).
#[allow(deprecated)]
use objc2_io_surface::{
    IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfaceIsGlobal,
    kIOSurfacePixelFormat, kIOSurfaceWidth,
};
use objc2_metal::*;

/// `'RGBA'`: 8-bit RGBA in memory order, matching `R8G8B8A8_UNORM`.
const PIXEL_FORMAT_RGBA: i32 = 0x5247_4241;

/// Wrap one slot surface as an arena image on `gpu`.
fn adopt(
    gpu: &mut MetalDevice,
    surface: &IOSurfaceRef,
    usage: vk::ImageUsageFlags,
) -> Result<Image> {
    let size = [surface.width() as u32, surface.height() as u32];
    let info = format(vk::Format::R8G8B8A8_UNORM)?;
    // SAFETY: an RGBA8 2D descriptor matching the surface's extent.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            info.pixel,
            size[0] as usize,
            size[1] as usize,
            false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    descriptor.setStorageMode(MTLStorageMode::Shared);
    let texture = gpu
        .device
        .newTextureWithDescriptor_iosurface_plane(&descriptor, surface, 0)
        .ok_or_else(|| nil("IOSurface texture"))?;
    gpu.adopt_image(NativeImage {
        texture,
        info,
        size,
        levels: 1,
        usage,
        storage: None,
    })
}

/// The producer side: a ring of exported IOSurface slots.
pub struct ExportRing {
    ring: Ring,
    size: [u32; 2],
    slots: Vec<(IoSurface, Image)>,
    serial: u64,
}

impl ExportRing {
    /// A first-generation ring of `size` RGBA8 slots.
    pub fn new(gpu: &mut MetalDevice, size: [u32; 2]) -> Result<Self> {
        let mut ring = Self {
            ring: Ring::new(0),
            size: [0; 2],
            slots: Vec::new(),
            serial: 0,
        };
        ring.resize(gpu, size)?;
        Ok(ring)
    }
    /// A new generation with fresh surfaces; old slots retire after their last copy.
    pub fn resize(&mut self, gpu: &mut MetalDevice, size: [u32; 2]) -> Result<()> {
        if size.contains(&0) {
            return Err(GpuError::Contract("Empty shared surface".into()));
        }
        let mut slots = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let surface = create_surface(size)?;
            let usage = vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED;
            let image = adopt(gpu, &surface, usage)?;
            slots.push((surface, image));
        }
        for (_, image) in std::mem::replace(&mut self.slots, slots) {
            Device::retire(gpu, Resource::Image(image))?;
        }
        self.ring = Ring::new(self.ring.generation() + 1);
        self.size = size;
        Ok(())
    }
    /// The current generation as sent to the owner.
    pub fn desc(&self, gpu: &MetalDevice) -> SurfaceDesc {
        SurfaceDesc {
            generation: self.ring.generation(),
            width: self.size[0],
            height: self.size[1],
            format: SurfaceFormat::Rgba8Unorm,
            device: gpu.info.identity,
            process: std::process::id(),
            slots: self
                .slots
                .iter()
                .map(|(surface, _)| SlotDesc {
                    handle: u64::from(surface.id()),
                    size: surface.alloc_size() as u64,
                    memory_type: 0,
                })
                .collect(),
        }
    }
    /// Slot extent.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    /// Slots the producer may write now.
    pub fn free(&self) -> usize {
        self.ring.free()
    }
    /// Return a slot the owner released; stale or foreign frames are ignored.
    pub fn release(&mut self, frame: Frame) -> bool {
        self.ring.release(frame)
    }
    /// Copy a completed `R8G8B8A8_UNORM` image of the ring's size (transfer source)
    /// into a free slot and wait for it. None (nothing written) while the owner holds
    /// every other slot: skip this frame rather than block the simulation.
    pub fn publish(
        &mut self,
        gpu: &mut MetalDevice,
        source: Image,
        tick: u64,
    ) -> Result<Option<Frame>> {
        let Some(slot) = self.ring.begin() else {
            return Ok(None);
        };
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::CopyImage {
            source,
            destination: self.slots[slot as usize].1,
        });
        let copied = gpu.submit(rec.finish(), &[]);
        let done = copied.and_then(|point| Device::wait(gpu, point));
        if let Err(error) = done {
            self.ring.abandon(slot);
            return Err(error);
        }
        self.serial += 1;
        Ok(Some(self.ring.publish(slot, self.serial, tick)))
    }
    /// Retire every slot image (the surfaces stay alive while the owner holds them).
    pub fn close(self, gpu: &mut MetalDevice) -> Result<()> {
        for (_, image) in self.slots {
            Device::retire(gpu, Resource::Image(image))?;
        }
        Ok(())
    }
}

/// A global (ID-addressable) RGBA8 IOSurface; deprecated by Apple as insecure, used on
/// purpose with the Windows `Local\` section trust level documented in `shared`.
#[allow(deprecated)]
fn create_surface(size: [u32; 2]) -> Result<IoSurface> {
    // SAFETY: the keys are IOSurface's own static property names.
    let keys: [&CFString; 5] = unsafe {
        [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfacePixelFormat,
            kIOSurfaceIsGlobal,
        ]
    };
    let (width, height) = (
        CFNumber::new_i32(size[0] as i32),
        CFNumber::new_i32(size[1] as i32),
    );
    let (bytes, pixel) = (CFNumber::new_i32(4), CFNumber::new_i32(PIXEL_FORMAT_RGBA));
    let values: [&CFType; 5] = [&width, &height, &bytes, &pixel, CFBoolean::new(true)];
    let properties = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // SAFETY: a dictionary of documented IOSurface properties with matching value types.
    unsafe { IOSurfaceRef::new(properties.as_opaque()) }
        .map(IoSurface::new)
        .ok_or_else(|| nil("IOSurfaceCreate"))
}

/// The owner side: the producer's slots as sampled images on this device.
pub struct ImportedSurface {
    generation: u64,
    size: [u32; 2],
    images: Vec<Image>,
    /// Retains the surfaces for as long as the images exist.
    _surfaces: SharedSurface,
    owned: [bool; SLOTS],
}

impl ImportedSurface {
    /// Import every slot; the producer must use this physical GPU.
    pub fn new(gpu: &mut MetalDevice, surface: &SharedSurface) -> Result<Self> {
        let desc = surface.desc();
        if desc.device != gpu.info.identity || desc.format != SurfaceFormat::Rgba8Unorm {
            return Err(GpuError::External(
                "shared surface comes from another GPU or format".into(),
            ));
        }
        let shared = surface
            .try_clone()
            .map_err(|e| GpuError::External(e.to_string()))?;
        let usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC;
        let images = shared
            .surfaces()
            .iter()
            .map(|slot| adopt(gpu, slot, usage))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            generation: desc.generation,
            size: [desc.width, desc.height],
            images,
            _surfaces: shared,
            owned: [false; SLOTS],
        })
    }
    /// The imported generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Slot extent.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    /// A slot's image, to publish with `sampled_image` and sample.
    pub fn image(&self, slot: u32) -> Option<Image> {
        self.images.get(slot as usize).copied()
    }
    /// Whether the owner currently holds `slot`.
    pub fn owns(&self, slot: u32) -> bool {
        self.owned.get(slot as usize).copied().unwrap_or(false)
    }
    /// Take a delivered slot and/or give one back. On unified memory this only tracks
    /// ownership; release a slot after the owner's sampling of it completed.
    pub fn exchange(&mut self, acquire: Option<u32>, release: Option<u32>) -> Result<()> {
        for (slot, owned) in [(acquire, true), (release, false)] {
            if let Some(slot) = slot {
                let held = self
                    .owned
                    .get_mut(slot as usize)
                    .ok_or_else(|| GpuError::Contract(format!("No shared slot {slot}")))?;
                if *held == owned {
                    return Err(GpuError::Contract(format!(
                        "Shared slot {slot} is already {}",
                        if owned { "owned" } else { "released" }
                    )));
                }
                *held = owned;
            }
        }
        Ok(())
    }
    /// Read an owned slot's pixels (tests and captures); waits for the copy.
    pub fn read(&self, gpu: &mut MetalDevice, slot: u32) -> Result<Vec<u8>> {
        let image = self
            .image(slot)
            .filter(|_| self.owns(slot))
            .ok_or_else(|| GpuError::Contract(format!("Shared slot {slot} is not owned")))?;
        let bytes = u64::from(self.size[0]) * u64::from(self.size[1]) * 4;
        let readback = gpu.create_readback_buffer(bytes)?;
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::ReadImage {
            source: image,
            destination: readback,
        });
        let point = gpu.submit(rec.finish(), &[])?;
        Device::wait(gpu, point)?;
        let pixels = gpu.read(readback);
        Device::retire(gpu, Resource::Buffer(readback))?;
        pixels
    }
    /// Retire the imported images.
    pub fn close(self, gpu: &mut MetalDevice) -> Result<()> {
        for image in self.images {
            Device::retire(gpu, Resource::Image(image))?;
        }
        Ok(())
    }
}
