//! Shared surfaces on Metal (D19 embedded Play): the [`crate::shared`] protocol over
//! IOSurfaces. The producer's ring slots are global IOSurfaces (their IDs travel as
//! [`SlotDesc::handle`]); the owner looks them up ([`SharedSurface`]) and wraps them
//! as textures. Like the Vulkan rings, each side owns its slot textures and a private
//! command queue ([`Copier`]), takes the device by shared reference and cleans up on
//! drop. Unified memory needs no queue-family transfer: the producer waits for its
//! copy before sending `FrameReady`, and the owner releases a slot only after its own
//! sampling completed.
//!
//! The backend-neutral methods (`create`, `recreate`, `publish_image`, `read_pixels`,
//! `exchange`, `desc`) match `vulkan::external`'s, unsafe where Vulkan's are, so call
//! sites compile unchanged against `GpuDevice`.
use super::native::{nil, ns_error, pool};
use super::{MetalDevice, WAIT_TIMEOUT_MS};
use crate::shared::{
    DeviceIdentity, Frame, IoSurface, Ring, SLOTS, SharedSurface, SlotDesc, SurfaceDesc,
    SurfaceFormat,
};
use crate::{GpuError, Image, Queue, Result};
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
use std::ptr::NonNull;

/// `'RGBA'`: 8-bit RGBA in memory order, matching `R8G8B8A8_UNORM`.
const PIXEL_FORMAT_RGBA: i32 = 0x5247_4241;

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Encoder = ProtocolObject<dyn MTL4ComputeCommandEncoder>;

/// A private queue for slot copies. Each copy waits on the device's timelines for
/// everything already submitted, then signals and waits on its own event.
struct Copier {
    queue: Retained<ProtocolObject<dyn MTL4CommandQueue>>,
    allocator: Retained<ProtocolObject<dyn MTL4CommandAllocator>>,
    event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
    value: u64,
    /// Slot textures and transient readback buffers.
    residency: Retained<ProtocolObject<dyn MTLResidencySet>>,
}

impl Copier {
    /// `sources` also makes the device's own allocations resident (producer copies).
    fn new(gpu: &MetalDevice, sources: bool) -> Result<Self> {
        let device = &gpu.device;
        let queue = device
            .newMTL4CommandQueue()
            .ok_or_else(|| nil("MTL4 queue"))?;
        let residency = device
            .newResidencySetWithDescriptor_error(&MTLResidencySetDescriptor::new())
            .map_err(|e| ns_error("residency set", &e))?;
        queue.addResidencySet(&residency);
        if sources {
            queue.addResidencySet(&gpu.residency);
        }
        Ok(Self {
            queue,
            allocator: device
                .newCommandAllocator()
                .ok_or_else(|| nil("command allocator"))?,
            event: device.newSharedEvent().ok_or_else(|| nil("shared event"))?,
            value: 0,
            residency,
        })
    }
    fn hold(&self, allocation: &ProtocolObject<dyn MTLAllocation>) {
        self.residency.addAllocation(allocation);
        self.residency.commit();
    }
    fn release(&self, allocation: &ProtocolObject<dyn MTLAllocation>) {
        self.residency.removeAllocation(allocation);
        self.residency.commit();
    }
    /// Encode blits after all of `gpu`'s submitted work, commit and wait.
    fn copy(&mut self, gpu: &MetalDevice, encode: impl FnOnce(&Encoder)) -> Result<()> {
        // The previous copy completed (waited below), so the allocator is reusable.
        self.allocator.reset();
        let cmd = gpu
            .device
            .newCommandBuffer()
            .ok_or_else(|| nil("command buffer"))?;
        cmd.beginCommandBufferWithAllocator(&self.allocator);
        let Some(encoder) = cmd.computeCommandEncoder() else {
            cmd.endCommandBuffer();
            return Err(nil("compute encoder"));
        };
        encode(&encoder);
        encoder.endEncoding();
        cmd.endCommandBuffer();
        for queue in [Queue::Graphics, Queue::Compute] {
            let value = gpu.submitted[queue.index()];
            if value > 0 {
                self.queue.waitForEvent_value(
                    ProtocolObject::from_ref(&*gpu.events[queue.index()]),
                    value,
                );
            }
        }
        let buffers = [NonNull::from(&*cmd)];
        // SAFETY: one ended, never-committed command buffer; the array outlives the call.
        unsafe { self.queue.commit_count(NonNull::from(&buffers[0]), 1) };
        self.value += 1;
        let event = ProtocolObject::from_ref(&*self.event);
        self.queue.signalEvent_value(event, self.value);
        let value = self.value;
        let done = gpu.waits.measure(|| {
            self.event
                .waitUntilSignaledValue_timeoutMS(value, WAIT_TIMEOUT_MS)
        });
        if done {
            Ok(())
        } else {
            Err(GpuError::Backend(anyhow::anyhow!(
                "Shared-surface copy timed out after {WAIT_TIMEOUT_MS} ms"
            )))
        }
    }
}

/// An RGBA8 texture over one slot surface.
fn texture(gpu: &MetalDevice, surface: &IOSurfaceRef) -> Result<Texture> {
    // SAFETY: an RGBA8 2D descriptor matching the surface's extent.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::RGBA8Unorm,
            surface.width(),
            surface.height(),
            false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
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
    copier: Copier,
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
            copier: pool(|| Copier::new(gpu, true))?,
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
                    let target = texture(gpu, &surface)?;
                    Ok((surface, target))
                })
                .collect::<Result<Vec<_>>>()
        })?;
        for (_, old) in std::mem::replace(&mut self.slots, slots) {
            self.copier.release(ProtocolObject::from_ref(&*old));
        }
        for (_, target) in &self.slots {
            self.copier.hold(ProtocolObject::from_ref(&**target));
        }
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
    /// Copy a completed `R8G8B8A8_UNORM` transfer source into a free slot and wait.
    /// None (nothing written) while the owner holds every other slot, and while
    /// `source_size` differs from the ring's: Metal has no scaling blit, and a scaled
    /// pass needs a cooked pipeline, so a mid-resize frame keeps the last-good one.
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
        if raw.size != source_size || raw.info.pixel != MTLPixelFormat::RGBA8Unorm {
            return Err(GpuError::Contract(
                "shared publication needs an RGBA8 source of its stated size".into(),
            ));
        }
        if source_size != self.size {
            return Ok(None);
        }
        let Some(slot) = self.ring.begin() else {
            return Ok(None);
        };
        let (from, to) = (raw.texture.clone(), self.slots[slot as usize].1.clone());
        let copied = pool(|| {
            self.copier.copy(gpu, |encoder| {
                // SAFETY: equally sized RGBA8 textures (checked), both resident on
                // the copier's queue.
                unsafe {
                    encoder.copyFromTexture_sourceSlice_sourceLevel_toTexture_destinationSlice_destinationLevel_sliceCount_levelCount(
                        &from, 0, 0, &to, 0, 0, 1, 1,
                    );
                }
            })
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
    copier: Copier,
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
        let copier = pool(|| Copier::new(gpu, false))?;
        let textures = pool(|| {
            shared
                .surfaces()
                .iter()
                .map(|slot| texture(gpu, slot))
                .collect::<Result<Vec<_>>>()
        })?;
        for texture in &textures {
            copier.hold(ProtocolObject::from_ref(&**texture));
        }
        Ok(Self {
            generation: desc.generation,
            size: [desc.width, desc.height],
            textures,
            _surfaces: shared,
            owned: [false; SLOTS],
            copier,
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
    /// Read an owned slot as tightly packed RGBA8 rows (tests, screenshots).
    /// # Safety
    /// As Vulkan's `read`: this thread owns the copy queue.
    pub unsafe fn read_pixels(&mut self, gpu: &MetalDevice, slot: u32) -> Result<Vec<u8>> {
        if !self.owns(slot) {
            return Err(GpuError::Contract(format!(
                "slot {slot} is not acquired by this device"
            )));
        }
        let [width, height] = self.size.map(|v| v as usize);
        let row = width * 4;
        let source = self.textures[slot as usize].clone();
        pool(|| {
            let buffer = gpu
                .device
                .newBufferWithLength_options(row * height, MTLResourceOptions::StorageModeShared)
                .ok_or_else(|| nil("readback buffer"))?;
            self.copier.hold(ProtocolObject::from_ref(&*buffer));
            let copied = self.copier.copy(gpu, |encoder| {
                let extent = MTLSize {
                    width,
                    height,
                    depth: 1,
                };
                // SAFETY: the whole slot into a buffer of exactly its packed size.
                unsafe {
                    encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                        &source, 0, 0, MTLOrigin { x: 0, y: 0, z: 0 }, extent,
                        &buffer, 0, row, row * height,
                    );
                }
            });
            self.copier.release(ProtocolObject::from_ref(&*buffer));
            copied?;
            // SAFETY: the copy completed (waited) and nothing else writes the buffer;
            // a shared buffer's contents span its whole length.
            let bytes = unsafe {
                std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), buffer.length())
            };
            Ok(bytes.to_vec())
        })
    }
}
