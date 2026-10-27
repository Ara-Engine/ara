//! Shared surfaces on Metal (D19 embedded Play): the [`crate::shared`] protocol over
//! IOSurfaces. The producer's ring slots are global IOSurfaces (their IDs travel as
//! [`SlotDesc::handle`]); the owner looks them up ([`SharedSurface`]) and wraps them
//! as textures. Like the Vulkan rings, each side owns its slot textures and a private
//! copy queue ([`super::transfer`]), takes the device by shared reference and cleans
//! up on drop. Unified memory needs no queue-family transfer: the producer waits for its
//! copy before sending `FrameReady`, and the owner releases a slot only after its own
//! sampling completed.
//!
//! The backend-neutral methods (`create`, `recreate`, `publish_image`, `read_pixels`,
//! `exchange`, `desc`) match `vulkan::external`'s, unsafe where Vulkan's are, so call
//! sites compile unchanged against `GpuDevice`.
use super::MetalDevice;
use super::native::{nil, pool};
use super::transfer::Transfer;
use crate::shared::{
    DeviceIdentity, Frame, IoSurface, Ring, SLOTS, SharedSurface, SlotDesc, SurfaceDesc,
    SurfaceFormat,
};
use crate::{GpuError, Image, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
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

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

/// An RGBA8 texture over one slot surface.
fn texture(gpu: &MetalDevice, surface: &IOSurfaceRef, usage: MTLTextureUsage) -> Result<Texture> {
    // SAFETY: an RGBA8 2D descriptor matching the surface's extent.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::RGBA8Unorm,
            surface.width(),
            surface.height(),
            false,
        )
    };
    descriptor.setUsage(usage);
    descriptor.setStorageMode(MTLStorageMode::Shared);
    gpu.device
        .newTextureWithDescriptor_iosurface_plane(&descriptor, surface, 0)
        .ok_or_else(|| nil("IOSurface texture"))
}

/// The producer side: a ring of exported IOSurface slots.
pub struct ExportRing {
    ring: Ring,
    size: [u32; 2],
    slots: Vec<(IoSurface, Texture)>,
    identity: DeviceIdentity,
    serial: u64,
    transfer: Transfer,
}

impl ExportRing {
    /// A first-generation ring of `size` RGBA8 slots on `gpu`.
    /// # Safety
    /// The Vulkan contract (`gpu` outlives the ring; this thread owns its graphics
    /// queue). Metal retains everything it uses, so nothing further is required.
    pub unsafe fn create(gpu: &MetalDevice, size: [u32; 2]) -> Result<Self> {
        let mut ring = Self {
            ring: Ring::new(0),
            size: [0; 2],
            slots: Vec::new(),
            identity: gpu.info.identity,
            serial: 0,
            transfer: Transfer::new(gpu)?,
        };
        // SAFETY: forwarded caller contract.
        unsafe { ring.recreate(gpu, size)? };
        Ok(ring)
    }
    /// A new generation with fresh surfaces. The owner's imports of older ones stay
    /// valid (they retain the surfaces) but are never written again.
    /// # Safety
    /// As [`Self::create`], with the same `gpu`.
    pub unsafe fn recreate(&mut self, gpu: &MetalDevice, size: [u32; 2]) -> Result<()> {
        if size.contains(&0) || size.iter().any(|&v| v > gpu.info.max_image_dimension) {
            return Err(GpuError::Contract(format!(
                "shared surface size {size:?} is outside 1..={}",
                gpu.info.max_image_dimension
            )));
        }
        let slots = pool(|| {
            (0..SLOTS)
                .map(|_| {
                    let surface = create_surface(size)?;
                    // Scaled publication writes slots with a compute kernel.
                    let usage = MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite;
                    let target = texture(gpu, &surface, usage)?;
                    Ok((surface, target))
                })
                .collect::<Result<Vec<_>>>()
        })?;
        self.slots = slots;
        self.ring = Ring::new(self.ring.generation() + 1);
        self.size = size;
        Ok(())
    }
    /// The current generation as sent to the owner.
    pub fn desc(&self) -> SurfaceDesc {
        SurfaceDesc {
            generation: self.ring.generation(),
            width: self.size[0],
            height: self.size[1],
            format: SurfaceFormat::Rgba8Unorm,
            device: self.identity,
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
    /// Copy a completed colour image into a free slot, scaling it when its extent
    /// differs (a view resize in progress keeps publishing), and wait. None (nothing
    /// written) while the owner holds every other slot.
    /// # Safety
    /// As Vulkan's `publish_scaled`: `source` is a completed image on `gpu`.
    pub unsafe fn publish_image(
        &mut self,
        gpu: &MetalDevice,
        source: Image,
        source_size: [u32; 2],
        tick: u64,
    ) -> Result<Option<Frame>> {
        let raw = &gpu.images.get(source)?.raw;
        if raw.size != source_size || raw.info.depth {
            return Err(GpuError::Contract(
                "shared publication needs a colour source of its stated size".into(),
            ));
        }
        let Some(slot) = self.ring.begin() else {
            return Ok(None);
        };
        let (from, to) = (raw.texture.clone(), self.slots[slot as usize].1.clone());
        let copied = pool(|| {
            self.transfer
                .run(Some(gpu), |transfer, cmd| transfer.copy(cmd, &from, &to))
        });
        if let Err(error) = copied {
            self.ring.abandon(slot);
            return Err(error);
        }
        self.serial += 1;
        Ok(Some(self.ring.publish(slot, self.serial, tick)))
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

/// The owner side: the producer's slots as textures on this device.
pub struct ImportedSurface {
    generation: u64,
    size: [u32; 2],
    textures: Vec<Texture>,
    /// Retains the surfaces for as long as the textures exist.
    _surfaces: SharedSurface,
    owned: [bool; SLOTS],
    transfer: Transfer,
    /// The GPU, for readback buffers.
    device: Retained<ProtocolObject<dyn MTLDevice>>,
}

impl ImportedSurface {
    /// Import every slot; the producer must use this physical GPU.
    /// # Safety
    /// As [`ExportRing::create`].
    pub unsafe fn create(gpu: &MetalDevice, surface: &SharedSurface) -> Result<Self> {
        let desc = surface.desc();
        if desc.device != gpu.info.identity || desc.format != SurfaceFormat::Rgba8Unorm {
            return Err(GpuError::External(
                "shared surface comes from another GPU or format".into(),
            ));
        }
        let shared = surface
            .try_clone()
            .map_err(|e| GpuError::External(e.to_string()))?;
        let textures = pool(|| {
            shared
                .surfaces()
                .iter()
                .map(|slot| texture(gpu, slot, MTLTextureUsage::ShaderRead))
                .collect::<Result<Vec<_>>>()
        })?;
        Ok(Self {
            generation: desc.generation,
            size: [desc.width, desc.height],
            textures,
            _surfaces: shared,
            owned: [false; SLOTS],
            transfer: Transfer::new(gpu)?,
            device: gpu.device.clone(),
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
    /// A slot's native texture, for the Metal UI backend to sample.
    pub fn texture(&self, slot: u32) -> Option<&ProtocolObject<dyn MTLTexture>> {
        self.textures.get(slot as usize).map(|texture| &**texture)
    }
    /// Whether the owner currently holds `slot`.
    pub fn owns(&self, slot: u32) -> bool {
        self.owned.get(slot as usize).copied().unwrap_or(false)
    }
    /// Take a delivered slot and/or give one back. On unified memory this only tracks
    /// ownership; already-owned acquires and unowned releases are skipped, as on Vulkan.
    /// # Safety
    /// As Vulkan's `exchange`: this device's last use of `release` completed.
    pub unsafe fn exchange(&mut self, acquire: Option<u32>, release: Option<u32>) -> Result<()> {
        for slot in acquire.iter().chain(&release) {
            if *slot as usize >= SLOTS {
                return Err(GpuError::Contract(format!(
                    "slot {slot} is outside 0..{SLOTS}"
                )));
            }
        }
        if let Some(slot) = acquire {
            self.owned[slot as usize] = true;
        }
        if let Some(slot) = release {
            self.owned[slot as usize] = false;
        }
        Ok(())
    }
    /// Read an owned slot as tightly packed RGBA8 rows ([`Self::read`]).
    /// # Safety
    /// As [`Self::read`].
    pub unsafe fn read_pixels(&mut self, _gpu: &MetalDevice, slot: u32) -> Result<Vec<u8>> {
        // SAFETY: forwarded caller contract.
        unsafe { self.read(slot) }
    }
    /// Read an owned slot as tightly packed RGBA8 rows (tests, screenshots). The
    /// producer wrote it, so the copy waits on nothing of this process.
    /// # Safety
    /// As Vulkan's `read`: this thread owns the import's copy queue.
    pub unsafe fn read(&mut self, slot: u32) -> Result<Vec<u8>> {
        if !self.owns(slot) {
            return Err(GpuError::Contract(format!(
                "slot {slot} is not acquired by this device"
            )));
        }
        let [width, height] = self.size.map(|v| v as usize);
        let row = width * 4;
        let source = self.textures[slot as usize].clone();
        pool(|| {
            let buffer = self
                .device
                .newBufferWithLength_options(row * height, MTLResourceOptions::StorageModeShared)
                .ok_or_else(|| nil("readback buffer"))?;
            self.transfer.run(None, |_, cmd| {
                let blit = cmd.blitCommandEncoder().ok_or_else(|| nil("blit encoder"))?;
                let extent = MTLSize {
                    width,
                    height,
                    depth: 1,
                };
                // SAFETY: the whole slot into a buffer of exactly its packed size.
                unsafe {
                    blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                        &source, 0, 0, MTLOrigin { x: 0, y: 0, z: 0 }, extent,
                        &buffer, 0, row, row * height,
                    );
                }
                blit.endEncoding();
                Ok(())
            })?;
            // SAFETY: the copy completed (waited) and nothing else writes the buffer;
            // a shared buffer's contents span its whole length.
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
            };
            Ok(bytes.to_vec())
        })
    }
}
