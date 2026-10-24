//! Shared surfaces on Vulkan: a [`SLOTS`]-image ring exported as opaque NT handles
//! (`VK_KHR_external_memory_win32`) and imported on another device of the same GPU.
//! [`crate::shared`] holds the cross-process protocol.
//!
//! Ownership and layouts: between owners every slot is owned by
//! `VK_QUEUE_FAMILY_EXTERNAL` and ends in `TRANSFER_SRC_OPTIMAL`. The producer hands a
//! written slot over with a release that also transitions `TRANSFER_DST →
//! TRANSFER_SRC`, and the owner acquires it with the same layouts (validation does
//! not track layouts across the external boundary, so the transition must ride the
//! transfer for the owner's tracker to learn the layout). The owner hands it back
//! with `TRANSFER_SRC → TRANSFER_SRC` and the producer acquires likewise, so every
//! release matches its acquire. Each side waits for its work on a fence before
//! telling the other, so a release happens-before the next acquire. Both sides
//! create the images with identical parameters and dedicated allocations, as opaque
//! handles require.
use super::Context;
use crate::{
    GpuError, Result,
    shared::{
        DeviceIdentity, Frame, Ring, SLOTS, SharedSurface, SlotDesc, SurfaceDesc, SurfaceFormat,
    },
};
use ash::vk::{self, TaggedStructure};

const HANDLE_TYPE: vk::ExternalMemoryHandleTypeFlags =
    vk::ExternalMemoryHandleTypeFlags::OPAQUE_WIN32;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
const NONE: (vk::PipelineStageFlags2, vk::AccessFlags2) =
    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE);
const ALL: (vk::PipelineStageFlags2, vk::AccessFlags2) = (
    vk::PipelineStageFlags2::ALL_COMMANDS,
    vk::AccessFlags2::from_raw(
        vk::AccessFlags2::MEMORY_READ.as_raw() | vk::AccessFlags2::MEMORY_WRITE.as_raw(),
    ),
);
const COPY_WRITE: (vk::PipelineStageFlags2, vk::AccessFlags2) = (
    vk::PipelineStageFlags2::COPY,
    vk::AccessFlags2::TRANSFER_WRITE,
);
const COPY_READ: (vk::PipelineStageFlags2, vk::AccessFlags2) = (
    vk::PipelineStageFlags2::COPY,
    vk::AccessFlags2::TRANSFER_READ,
);
const SRC: vk::ImageLayout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;

/// Usage of every shared image, identical on both sides: copy target (producer),
/// copy source (readback, presentation) and sampled (the owner's UI).
fn usage() -> vk::ImageUsageFlags {
    vk::ImageUsageFlags::TRANSFER_SRC
        | vk::ImageUsageFlags::TRANSFER_DST
        | vk::ImageUsageFlags::SAMPLED
}

fn external(message: impl Into<String>) -> GpuError {
    GpuError::External(message.into())
}

fn backend(error: vk::Result) -> GpuError {
    GpuError::Backend(error.into())
}

/// Whether `gpu` can export and import shared surfaces.
pub fn supported(gpu: &Context) -> bool {
    cfg!(windows) && gpu.info.external_memory
}

/// Fails with `ARA_GPU_EXTERNAL` unless `gpu` can export (or import) RGBA8 images.
fn require(gpu: &Context, export: bool) -> Result<()> {
    if !cfg!(windows) {
        return Err(external(
            "shared surfaces use opaque NT handles; this platform is unsupported",
        ));
    }
    if !gpu.info.external_memory {
        return Err(external(format!(
            "`{}` does not offer VK_KHR_external_memory_win32",
            gpu.info.name
        )));
    }
    let mut external_info =
        vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(HANDLE_TYPE);
    let info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(FORMAT)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage())
        .push(&mut external_info);
    let mut properties = vk::ExternalImageFormatProperties::default();
    {
        let mut out = vk::ImageFormatProperties2::default().push(&mut properties);
        // SAFETY: Read-only capability query on a live instance/physical device.
        unsafe {
            gpu.instance
                .get_physical_device_image_format_properties2(gpu.physical, &info, &mut out)
        }
        .map_err(|e| external(format!("RGBA8 images cannot be shared on this device: {e}")))?;
    }
    let needed = if export {
        vk::ExternalMemoryFeatureFlags::EXPORTABLE
    } else {
        vk::ExternalMemoryFeatureFlags::IMPORTABLE
    };
    if !properties
        .external_memory_properties
        .external_memory_features
        .contains(needed)
    {
        return Err(external(format!(
            "the driver cannot {} opaque NT-handle RGBA8 images",
            if export { "export" } else { "import" }
        )));
    }
    Ok(())
}

/// Creates a shared image (identical parameters on both sides).
/// # Safety
/// `device` is live; the caller destroys the image.
unsafe fn create_image(device: &ash::Device, size: [u32; 2]) -> Result<vk::Image> {
    let mut external_info = vk::ExternalMemoryImageCreateInfo::default().handle_types(HANDLE_TYPE);
    // SAFETY: Create info and its chain live through the call.
    unsafe {
        device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(FORMAT)
                .extent(vk::Extent3D {
                    width: size[0],
                    height: size[1],
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage())
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .push(&mut external_info),
            None,
        )
    }
    .map_err(backend)
}

fn color() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

/// One image barrier, optionally moving queue-family ownership.
fn barrier(
    image: vk::Image,
    layouts: (vk::ImageLayout, vk::ImageLayout),
    families: (u32, u32),
    src: (vk::PipelineStageFlags2, vk::AccessFlags2),
    dst: (vk::PipelineStageFlags2, vk::AccessFlags2),
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .image(image)
        .subresource_range(color())
        .old_layout(layouts.0)
        .new_layout(layouts.1)
        .src_queue_family_index(families.0)
        .dst_queue_family_index(families.1)
        .src_stage_mask(src.0)
        .src_access_mask(src.1)
        .dst_stage_mask(dst.0)
        .dst_access_mask(dst.1)
}

fn same() -> (u32, u32) {
    (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED)
}

/// One graphics command buffer with a fence: record, submit, wait.
struct Commands {
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    in_flight: bool,
}

impl Commands {
    /// # Safety
    /// `gpu` outlives the result; its graphics queue is used from this thread only.
    unsafe fn new(gpu: &Context) -> Result<Self> {
        let mut commands = Self {
            device: gpu.device.clone(),
            queue: gpu.graphics,
            pool: vk::CommandPool::null(),
            command: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            in_flight: false,
        };
        // SAFETY: Live device; Drop destroys whatever was created if a later step fails.
        unsafe {
            commands.pool = gpu
                .device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(gpu.info.graphics_family)
                        .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                    None,
                )
                .map_err(backend)?;
            commands.command = gpu
                .device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(commands.pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .map_err(backend)?[0];
            commands.fence = gpu
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(backend)?;
        }
        Ok(commands)
    }

    /// Records with `record`, submits on the graphics queue and waits for completion.
    /// # Safety
    /// Everything `record` references is live; no other thread uses the queue.
    unsafe fn run(&mut self, record: impl FnOnce(&ash::Device, vk::CommandBuffer)) -> Result<()> {
        let device = &self.device;
        // SAFETY: The previous run waited for its fence, so the pool is idle.
        unsafe {
            device
                .reset_command_pool(self.pool, vk::CommandPoolResetFlags::empty())
                .map_err(backend)?;
            device
                .begin_command_buffer(
                    self.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(backend)?;
            record(device, self.command);
            device.end_command_buffer(self.command).map_err(backend)?;
            device.reset_fences(&[self.fence]).map_err(backend)?;
            let buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(self.command)];
            device
                .queue_submit2(
                    self.queue,
                    &[vk::SubmitInfo2::default().command_buffer_infos(&buffers)],
                    self.fence,
                )
                .map_err(backend)?;
            self.in_flight = true;
            device
                .wait_for_fences(&[self.fence], true, u64::MAX)
                .map_err(backend)?;
            self.in_flight = false;
        }
        Ok(())
    }
}

impl Drop for Commands {
    fn drop(&mut self) {
        // SAFETY: Completed work only, except after a failed wait (device loss), when
        // the device is idled first. Null handles are skipped.
        unsafe {
            if self.in_flight {
                let _ = self.device.device_wait_idle();
            }
            if self.fence != vk::Fence::null() {
                self.device.destroy_fence(self.fence, None);
            }
            if self.pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.pool, None);
            }
        }
    }
}

#[cfg(windows)]
type Handle = std::os::windows::io::OwnedHandle;
#[cfg(not(windows))]
type Handle = ();

#[cfg(windows)]
fn raw(handle: &Handle) -> u64 {
    use std::os::windows::io::AsRawHandle;
    handle.as_raw_handle() as u64
}
#[cfg(not(windows))]
fn raw(_: &Handle) -> u64 {
    0
}

/// One exported slot: its image, dedicated memory and this process's NT handle.
struct Exported {
    device: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
    size: u64,
    memory_type: u32,
    handle: Option<Handle>,
}

impl Drop for Exported {
    fn drop(&mut self) {
        // SAFETY: The ring's work completed (every publish waits). Importers keep the
        // payload alive through their own memory objects and handles.
        unsafe {
            if self.image != vk::Image::null() {
                self.device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}

/// Allocates one exportable slot.
/// # Safety
/// `gpu` is live and outlives the slot.
unsafe fn export_slot(gpu: &Context, size: [u32; 2]) -> Result<Exported> {
    let mut slot = Exported {
        device: gpu.device.clone(),
        image: vk::Image::null(),
        memory: vk::DeviceMemory::null(),
        size: 0,
        memory_type: 0,
        handle: None,
    };
    // SAFETY: Allocation follows the queried requirements; `slot` frees partial work.
    unsafe {
        slot.image = create_image(&gpu.device, size)?;
        let requirements = gpu.device.get_image_memory_requirements(slot.image);
        slot.memory_type = gpu
            .memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .map_err(GpuError::Backend)?;
        slot.size = requirements.size;
        let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(HANDLE_TYPE);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(slot.image);
        slot.memory = gpu
            .device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(slot.size)
                    .memory_type_index(slot.memory_type)
                    .push(&mut export)
                    .push(&mut dedicated),
                None,
            )
            .map_err(backend)?;
        gpu.device
            .bind_image_memory(slot.image, slot.memory, 0)
            .map_err(backend)?;
        slot.handle = Some(export_handle(gpu, slot.memory)?);
    }
    Ok(slot)
}

/// An unnamed NT handle to `memory` (default security: only this user's processes
/// that can duplicate handles out of this process can reach it).
#[cfg(windows)]
unsafe fn export_handle(gpu: &Context, memory: vk::DeviceMemory) -> Result<Handle> {
    use std::os::windows::io::FromRawHandle;
    let win32 = ash::khr::external_memory_win32::Device::load(&gpu.instance, &gpu.device);
    // SAFETY: `memory` was allocated exportable as OPAQUE_WIN32 on this device.
    let handle = unsafe {
        win32.get_memory_win32_handle(
            &vk::MemoryGetWin32HandleInfoKHR::default()
                .memory(memory)
                .handle_type(HANDLE_TYPE),
        )
    }
    .map_err(backend)?;
    if handle == 0 {
        return Err(external("the driver returned a null memory handle"));
    }
    // SAFETY: vkGetMemoryWin32HandleKHR returns a new NT handle the caller owns.
    Ok(unsafe { Handle::from_raw_handle(handle as std::os::windows::io::RawHandle) })
}
#[cfg(not(windows))]
unsafe fn export_handle(_: &Context, _: vk::DeviceMemory) -> Result<Handle> {
    Err(external(
        "shared surfaces use opaque NT handles; this platform is unsupported",
    ))
}

/// The producer's ring: renders are copied into free slots, published as [`Frame`]s
/// and written again only after the owner released them ([`Ring`]).
pub struct ExportRing {
    slots: Vec<Exported>,
    ring: Ring,
    /// The slot was handed out before, so it rests with the external owner and the
    /// next write acquires it first.
    shared: [bool; SLOTS],
    size: [u32; 2],
    family: u32,
    identity: DeviceIdentity,
    serial: u64,
    commands: Commands,
}

impl ExportRing {
    /// A first-generation ring of `size` RGBA8 slots on `gpu`.
    /// # Safety
    /// `gpu` outlives the ring; its graphics queue is used from this thread only.
    pub unsafe fn new(gpu: &Context, size: [u32; 2]) -> Result<Self> {
        require(gpu, true)?;
        let mut ring = Self {
            slots: Vec::new(),
            ring: Ring::new(0),
            shared: [false; SLOTS],
            size,
            family: gpu.info.graphics_family,
            identity: gpu.info.identity,
            serial: 0,
            // SAFETY: Forwarded caller contract.
            commands: unsafe { Commands::new(gpu)? },
        };
        // SAFETY: Forwarded caller contract.
        unsafe { ring.resize(gpu, size)? };
        Ok(ring)
    }

    /// Replaces every slot with new `size` images: the next generation. The owner's
    /// imports of older generations stay valid (they hold the memory) but are never
    /// written again.
    /// # Safety
    /// As [`ExportRing::new`], with the same `gpu`.
    pub unsafe fn resize(&mut self, gpu: &Context, size: [u32; 2]) -> Result<()> {
        if size.contains(&0)
            || size[0] > gpu.info.max_image_dimension
            || size[1] > gpu.info.max_image_dimension
        {
            return Err(GpuError::Contract(format!(
                "shared surface size {size:?} is outside 1..={}",
                gpu.info.max_image_dimension
            )));
        }
        let slots = (0..SLOTS)
            // SAFETY: Forwarded caller contract.
            .map(|_| unsafe { export_slot(gpu, size) })
            .collect::<Result<Vec<_>>>()?;
        self.slots = slots;
        self.ring = Ring::new(self.ring.generation() + 1);
        self.shared = [false; SLOTS];
        self.size = size;
        Ok(())
    }

    /// The current generation, with this process's handle values.
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
                .map(|s| SlotDesc {
                    handle: s.handle.as_ref().map_or(0, raw),
                    size: s.size,
                    memory_type: s.memory_type,
                })
                .collect(),
        }
    }

    /// Slot size in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Slots the producer may write now.
    pub fn free(&self) -> usize {
        self.ring.free()
    }

    /// The owner returned `frame`; false for a stale or foreign frame (ignored).
    pub fn release(&mut self, frame: Frame) -> bool {
        self.ring.release(frame)
    }

    /// Copies `source` into a free slot, waits for the copy and publishes it as the
    /// frame showing `tick`. `None` (nothing recorded) when the owner holds every
    /// other slot: skip this frame.
    /// # Safety
    /// `source` is an RGBA8 image of [`ExportRing::size`] on the same device, in
    /// `TRANSFER_SRC_OPTIMAL`, whose writes were submitted earlier on the graphics
    /// queue; this thread owns that queue.
    pub unsafe fn publish(&mut self, source: vk::Image, tick: u64) -> Result<Option<Frame>> {
        let Some(slot) = self.ring.begin() else {
            return Ok(None);
        };
        let index = slot as usize;
        let image = self.slots[index].image;
        let shared = self.shared[index];
        let (family, size) = (self.family, self.size);
        // SAFETY: Caller contract for `source`; the slot image is ours (Writing).
        let result = unsafe {
            self.commands.run(|device, command| {
                let barriers = |list: &[vk::ImageMemoryBarrier2<'_>]| {
                    device.cmd_pipeline_barrier2(
                        command,
                        &vk::DependencyInfo::default().image_memory_barriers(list),
                    );
                };
                if shared {
                    // Acquire from the owner, matching its TRANSFER_SRC release.
                    barriers(&[barrier(
                        image,
                        (SRC, SRC),
                        (vk::QUEUE_FAMILY_EXTERNAL, family),
                        NONE,
                        ALL,
                    )]);
                }
                barriers(&[
                    // Contents are replaced: discard them.
                    barrier(
                        image,
                        (
                            vk::ImageLayout::UNDEFINED,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        ),
                        same(),
                        if shared { ALL } else { NONE },
                        COPY_WRITE,
                    ),
                    barrier(source, (SRC, SRC), same(), ALL, COPY_READ),
                ]);
                device.cmd_copy_image(
                    command,
                    source,
                    SRC,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[vk::ImageCopy::default()
                        .src_subresource(layers())
                        .dst_subresource(layers())
                        .extent(vk::Extent3D {
                            width: size[0],
                            height: size[1],
                            depth: 1,
                        })],
                );
                // Release to the owner; the transition to the resting layout rides
                // the transfer (the owner's acquire names the same layouts).
                barriers(&[barrier(
                    image,
                    (vk::ImageLayout::TRANSFER_DST_OPTIMAL, SRC),
                    (family, vk::QUEUE_FAMILY_EXTERNAL),
                    COPY_WRITE,
                    NONE,
                )]);
            })
        };
        if let Err(error) = result {
            self.ring.abandon(slot);
            return Err(error);
        }
        self.shared[index] = true;
        self.serial += 1;
        Ok(Some(self.ring.publish(slot, self.serial, tick)))
    }
}

fn layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1)
}

/// One imported slot.
struct Imported {
    device: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

impl Drop for Imported {
    fn drop(&mut self) {
        // SAFETY: The surface released and idled its work before dropping slots.
        unsafe {
            if self.view != vk::ImageView::null() {
                self.device.destroy_image_view(self.view, None);
            }
            if self.image != vk::Image::null() {
                self.device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}

/// The owner's import of one surface generation on its own device.
pub struct ImportedSurface {
    slots: Vec<Imported>,
    /// Slots this device's graphics family owns (acquired, `TRANSFER_SRC_OPTIMAL`).
    owned: [bool; SLOTS],
    family: u32,
    generation: u64,
    size: [u32; 2],
    memory: vk::PhysicalDeviceMemoryProperties,
    commands: Commands,
}

impl ImportedSurface {
    /// Imports every slot of `surface` (handles valid in this process) on `gpu`.
    /// Fails with `ARA_GPU_EXTERNAL` when `gpu` is another GPU or driver than the
    /// producer's, or cannot import.
    /// # Safety
    /// `gpu` outlives the surface; its graphics queue is used from this thread only.
    pub unsafe fn new(gpu: &Context, surface: &SharedSurface) -> Result<Self> {
        let desc = surface.desc();
        require(gpu, false)?;
        let own = gpu.info.identity;
        let luid_differs = matches!((own.luid, desc.device.luid), (Some(a), Some(b)) if a != b);
        if own.device_uuid != desc.device.device_uuid
            || own.driver_uuid != desc.device.driver_uuid
            || luid_differs
        {
            return Err(external(format!(
                "the surface lives on another GPU or driver (device {}, driver {}) than `{}` \
                 (device {}, driver {}); run Play and the editor on the same adapter",
                hex(&desc.device.device_uuid),
                hex(&desc.device.driver_uuid),
                gpu.info.name,
                hex(&own.device_uuid),
                hex(&own.driver_uuid)
            )));
        }
        if desc.process != std::process::id() {
            return Err(GpuError::Contract(format!(
                "surface handles belong to process {}; duplicate them into this one first",
                desc.process
            )));
        }
        if desc.slots.len() != SLOTS || desc.format != SurfaceFormat::Rgba8Unorm {
            return Err(GpuError::Contract(format!(
                "a shared surface is {SLOTS} RGBA8 slots, got {} {:?}",
                desc.slots.len(),
                desc.format
            )));
        }
        let size = [desc.width, desc.height];
        if size.contains(&0)
            || size[0] > gpu.info.max_image_dimension
            || size[1] > gpu.info.max_image_dimension
        {
            return Err(GpuError::Contract(format!(
                "shared surface size {size:?} is outside device limits"
            )));
        }
        let mut imported = Self {
            slots: Vec::new(),
            owned: [false; SLOTS],
            family: gpu.info.graphics_family,
            generation: desc.generation,
            size,
            // SAFETY: Read-only query on a live physical device.
            memory: unsafe {
                gpu.instance
                    .get_physical_device_memory_properties(gpu.physical)
            },
            // SAFETY: Forwarded caller contract.
            commands: unsafe { Commands::new(gpu)? },
        };
        for slot in &desc.slots {
            // SAFETY: Forwarded caller contract; the handle is valid in this process.
            let slot = unsafe { import_slot(gpu, size, slot)? };
            imported.slots.push(slot);
        }
        Ok(imported)
    }

    /// The imported generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Slot size in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// A slot's image and sampled view (RGBA8 UNORM).
    pub fn image(&self, slot: u32) -> Option<(vk::Image, vk::ImageView)> {
        self.slots.get(slot as usize).map(|s| (s.image, s.view))
    }

    /// Whether this device owns `slot` (acquired, in `TRANSFER_SRC_OPTIMAL`).
    pub fn owns(&self, slot: u32) -> bool {
        self.owned.get(slot as usize).copied().unwrap_or(false)
    }

    /// Moves slots between the producer and this device in one submission, and waits
    /// for it: `release` goes back to the producer, `acquire` comes to this device's
    /// graphics family in `TRANSFER_SRC_OPTIMAL`. Already-owned acquires and unowned
    /// releases are skipped.
    /// # Safety
    /// This device's last use of `release` completed; the producer published
    /// `acquire` (its release happened before); this thread owns the queue.
    pub unsafe fn exchange(&mut self, acquire: Option<u32>, release: Option<u32>) -> Result<()> {
        for slot in acquire.iter().chain(&release) {
            if *slot as usize >= SLOTS {
                return Err(GpuError::Contract(format!(
                    "slot {slot} is outside 0..{SLOTS}"
                )));
            }
        }
        let release = release.filter(|s| self.owned[*s as usize] && Some(*s) != acquire);
        let acquire = acquire.filter(|s| !self.owned[*s as usize]);
        let mut barriers = Vec::new();
        if let Some(slot) = release {
            barriers.push(barrier(
                self.slots[slot as usize].image,
                (SRC, SRC),
                (self.family, vk::QUEUE_FAMILY_EXTERNAL),
                ALL,
                NONE,
            ));
        }
        if let Some(slot) = acquire {
            // Matches the producer's release, transition included.
            barriers.push(barrier(
                self.slots[slot as usize].image,
                (vk::ImageLayout::TRANSFER_DST_OPTIMAL, SRC),
                (vk::QUEUE_FAMILY_EXTERNAL, self.family),
                NONE,
                ALL,
            ));
        }
        if barriers.is_empty() {
            return Ok(());
        }
        // SAFETY: Caller contract; the barriers describe the protocol's resting state.
        unsafe {
            self.commands.run(|device, command| {
                device.cmd_pipeline_barrier2(
                    command,
                    &vk::DependencyInfo::default().image_memory_barriers(&barriers),
                );
            })?;
        }
        if let Some(slot) = release {
            self.owned[slot as usize] = false;
        }
        if let Some(slot) = acquire {
            self.owned[slot as usize] = true;
        }
        Ok(())
    }

    /// Reads an owned slot back as tightly packed RGBA8 rows (tests, screenshots).
    /// # Safety
    /// This thread owns the graphics queue.
    pub unsafe fn read(&mut self, slot: u32) -> Result<Vec<u8>> {
        if !self.owns(slot) {
            return Err(GpuError::Contract(format!(
                "slot {slot} is not acquired by this device"
            )));
        }
        let image = self.slots[slot as usize].image;
        let bytes = u64::from(self.size[0]) * u64::from(self.size[1]) * 4;
        let device = self.commands.device.clone();
        let mut buffer = Readback {
            device: device.clone(),
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
        };
        let size = self.size;
        // SAFETY: Host-visible coherent buffer sized for the copy; waited before mapping.
        unsafe {
            buffer.buffer = device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(bytes)
                        .usage(vk::BufferUsageFlags::TRANSFER_DST)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .map_err(backend)?;
            let requirements = device.get_buffer_memory_requirements(buffer.buffer);
            let flags =
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let memory_type = (0..self.memory.memory_type_count)
                .find(|i| {
                    requirements.memory_type_bits & (1 << i) != 0
                        && self.memory.memory_types[*i as usize]
                            .property_flags
                            .contains(flags)
                })
                .ok_or_else(|| external("no host-visible memory for a readback"))?;
            buffer.memory = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type),
                    None,
                )
                .map_err(backend)?;
            device
                .bind_buffer_memory(buffer.buffer, buffer.memory, 0)
                .map_err(backend)?;
            let target = buffer.buffer;
            self.commands.run(|device, command| {
                device.cmd_pipeline_barrier2(
                    command,
                    &vk::DependencyInfo::default().image_memory_barriers(&[barrier(
                        image,
                        (SRC, SRC),
                        same(),
                        ALL,
                        COPY_READ,
                    )]),
                );
                device.cmd_copy_image_to_buffer(
                    command,
                    image,
                    SRC,
                    target,
                    &[vk::BufferImageCopy::default()
                        .image_subresource(layers())
                        .image_extent(vk::Extent3D {
                            width: size[0],
                            height: size[1],
                            depth: 1,
                        })],
                );
                device.cmd_pipeline_barrier2(
                    command,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&[
                        vk::BufferMemoryBarrier2::default()
                            .buffer(target)
                            .size(bytes)
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .src_stage_mask(vk::PipelineStageFlags2::COPY)
                            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                            .dst_access_mask(vk::AccessFlags2::HOST_READ),
                    ]),
                );
            })?;
            let pointer = device
                .map_memory(buffer.memory, 0, bytes, vk::MemoryMapFlags::empty())
                .map_err(backend)?;
            let pixels = std::slice::from_raw_parts(pointer.cast::<u8>(), bytes as usize).to_vec();
            device.unmap_memory(buffer.memory);
            Ok(pixels)
        }
    }
}

impl Drop for ImportedSurface {
    fn drop(&mut self) {
        // Hand acquired slots back so the producer's next acquire has its release.
        let owned: Vec<u32> = (0..SLOTS as u32).filter(|s| self.owns(*s)).collect();
        for slot in owned {
            // SAFETY: The surface's own work completed (every run waits); the owner's
            // other uses (UI draws) completed before it dropped the surface.
            let _ = unsafe { self.exchange(None, Some(slot)) };
        }
    }
}

/// A temporary host-visible readback buffer.
struct Readback {
    device: ash::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

impl Drop for Readback {
    fn drop(&mut self) {
        // SAFETY: The copy into it was waited for (or never submitted).
        unsafe {
            if self.buffer != vk::Buffer::null() {
                self.device.destroy_buffer(self.buffer, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
            }
        }
    }
}

/// Imports one slot: an identical image bound to the producer's memory payload.
/// # Safety
/// `gpu` is live; `slot.handle` is a valid NT handle in this process.
unsafe fn import_slot(gpu: &Context, size: [u32; 2], slot: &SlotDesc) -> Result<Imported> {
    let mut imported = Imported {
        device: gpu.device.clone(),
        image: vk::Image::null(),
        memory: vk::DeviceMemory::null(),
        view: vk::ImageView::null(),
    };
    // SAFETY: Requirements are checked against the exported allocation; `imported`
    // frees partial work. Importing does not take ownership of the NT handle.
    unsafe {
        imported.image = create_image(&gpu.device, size)?;
        let requirements = gpu.device.get_image_memory_requirements(imported.image);
        if slot.memory_type >= 32
            || requirements.memory_type_bits & (1 << slot.memory_type) == 0
            || requirements.size > slot.size
        {
            return Err(external(format!(
                "exported memory (type {}, {} bytes) does not fit this device's image \
                 (types {:#b}, {} bytes)",
                slot.memory_type, slot.size, requirements.memory_type_bits, requirements.size
            )));
        }
        let mut import = vk::ImportMemoryWin32HandleInfoKHR::default()
            .handle_type(HANDLE_TYPE)
            .handle(slot.handle as vk::HANDLE);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(imported.image);
        imported.memory = gpu
            .device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(slot.size)
                    .memory_type_index(slot.memory_type)
                    .push(&mut import)
                    .push(&mut dedicated),
                None,
            )
            .map_err(|e| external(format!("importing a surface slot failed: {e}")))?;
        gpu.device
            .bind_image_memory(imported.image, imported.memory, 0)
            .map_err(backend)?;
        imported.view = gpu
            .device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(imported.image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(FORMAT)
                    .subresource_range(color()),
                None,
            )
            .map_err(backend)?;
    }
    Ok(imported)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
