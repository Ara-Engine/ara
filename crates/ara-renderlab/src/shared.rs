//! The zero-copy handoff: Renderlab's frame reaches the presenter's device through
//! images the two devices share (opaque NT handles, so Windows only; other
//! platforms and mismatched adapters keep the CPU copy in `bridge`).
//!
//! Renderlab's device exports a ring of [`SLOTS`] RGBA8 images exactly as
//! `ara_gpu::vulkan::external::ExportRing` does, so the presenter's device imports
//! them with the engine's own `ImportedSurface` and the documented ownership
//! protocol holds unchanged: between owners a slot belongs to
//! `VK_QUEUE_FAMILY_EXTERNAL` in `TRANSFER_SRC_OPTIMAL`, the producer's release
//! carries `TRANSFER_DST -> TRANSFER_SRC`, and each side waits for its work before
//! the other touches the slot. Only slot 0 is ever written: the presenter copies it
//! out and hands it back within the frame, so there is nothing to rotate.
//!
//! Slot 0 is also a wgpu texture, so the frame is copied into it by wgpu (which
//! tracks it in `COPY_DST`, that is `TRANSFER_DST_OPTIMAL`); the ownership barriers
//! around that copy are raw Vulkan on the same queue and leave the image in the
//! layout wgpu expects.
use anyhow::{Context as _, Result, bail};
use ara_gpu::{
    GpuDevice, Image,
    shared::{DeviceIdentity, SLOTS, SharedSurface, SlotDesc, SurfaceDesc, SurfaceFormat},
    vulkan::external::ImportedSurface,
};
use ash038::{self as ash, vk};
use renderlab::engine::Engine;

const HANDLE_TYPE: vk::ExternalMemoryHandleTypeFlags =
    vk::ExternalMemoryHandleTypeFlags::OPAQUE_WIN32;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

#[cfg(windows)]
type Handle = std::os::windows::io::OwnedHandle;
#[cfg(not(windows))]
/// No handle exists off Windows: `SharedHandoff::new` fails before any export.
struct Handle;

/// One exported image with its dedicated memory; destroyed with the value.
struct Slot {
    device: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
}

impl Drop for Slot {
    fn drop(&mut self) {
        // SAFETY: Every use of the slot was waited for (each publish waits its fence),
        // and the device outlives it: spare slots drop before the engine, and slot 0
        // drops from wgpu's own texture destruction.
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

/// The shared ring on Renderlab's device and its import on the presenter's.
pub(crate) struct SharedHandoff {
    raw: ash::Device,
    queue: vk::Queue,
    family: u32,
    pool: vk::CommandPool,
    /// Recorded per frame: the slot's acquire before the copy, its release after.
    acquire: vk::CommandBuffer,
    release: vk::CommandBuffer,
    fence: vk::Fence,
    /// Slot 0 as wgpu sees it; wgpu destroys the slot when it destroys the texture.
    texture: wgpu::Texture,
    image: vk::Image,
    /// The ring's other slots: imported, never written.
    _spare: Vec<Slot>,
    _handles: Vec<Handle>,
    _surface: SharedSurface,
    imported: ImportedSurface,
    extent: wgpu::Extent3d,
    /// Slot 0 rests with the presenter's side: the next frame acquires it first.
    handed_over: bool,
    /// A release was submitted and not yet waited for.
    in_flight: bool,
}

fn subresource() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

/// A classic image barrier (Renderlab's device may not enable synchronization2).
fn barrier(
    image: vk::Image,
    layouts: (vk::ImageLayout, vk::ImageLayout),
    families: (u32, u32),
    access: (vk::AccessFlags, vk::AccessFlags),
) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .image(image)
        .subresource_range(subresource())
        .old_layout(layouts.0)
        .new_layout(layouts.1)
        .src_queue_family_index(families.0)
        .dst_queue_family_index(families.1)
        .src_access_mask(access.0)
        .dst_access_mask(access.1)
}

impl SharedHandoff {
    /// Exports a ring of `size` on the engine's device and imports it on `gpu`.
    /// Fails (the caller falls back to the CPU copy) off Windows, when the two
    /// devices are different adapters, or when the driver cannot share RGBA8 images.
    pub fn new(engine: &Engine, gpu: &GpuDevice, size: [u32; 2]) -> Result<Self> {
        if !cfg!(windows) {
            bail!("shared images use opaque NT handles (Windows only)");
        }
        let device = engine.device();
        // SAFETY: The handle is used only to read the device's raw objects while the
        // engine (and so the device) is alive; nothing is destroyed through it.
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }
            .context("Renderlab's device is not a Vulkan device")?;
        if !hal
            .enabled_device_extensions()
            .contains(&ash::khr::external_memory_win32::NAME)
        {
            bail!("Renderlab's device does not have VK_KHR_external_memory_win32");
        }
        let raw = hal.raw_device().clone();
        let instance = hal.shared_instance().raw_instance();
        let physical = hal.raw_physical_device();
        let family = hal.queue_family_index();
        let queue = hal.raw_queue();

        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
        // SAFETY: Read-only queries on a live instance and physical device.
        let memory_properties = unsafe {
            instance.get_physical_device_properties2(physical, &mut properties);
            instance.get_physical_device_memory_properties(physical)
        };
        let identity = DeviceIdentity {
            device_uuid: id.device_uuid,
            driver_uuid: id.driver_uuid,
            luid: (id.device_luid_valid == vk::TRUE).then_some(id.device_luid),
        };

        let win32 = ash::khr::external_memory_win32::Device::new(instance, &raw);
        let mut slots = Vec::with_capacity(SLOTS);
        let mut handles = Vec::with_capacity(SLOTS);
        let mut described = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let mut slot = Slot {
                device: raw.clone(),
                image: vk::Image::null(),
                memory: vk::DeviceMemory::null(),
            };
            // SAFETY: Creation follows the queried requirements; `slot` frees partial
            // work. The parameters equal `ara_gpu`'s shared images, as opaque handles
            // require of both sides.
            let (bytes, memory_type, handle) = unsafe {
                let mut external =
                    vk::ExternalMemoryImageCreateInfo::default().handle_types(HANDLE_TYPE);
                slot.image = raw.create_image(
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
                        .usage(
                            vk::ImageUsageFlags::TRANSFER_SRC
                                | vk::ImageUsageFlags::TRANSFER_DST
                                | vk::ImageUsageFlags::SAMPLED,
                        )
                        .sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .initial_layout(vk::ImageLayout::UNDEFINED)
                        .push_next(&mut external),
                    None,
                )?;
                let requirements = raw.get_image_memory_requirements(slot.image);
                let memory_type = (0..memory_properties.memory_type_count)
                    .find(|i| {
                        requirements.memory_type_bits & (1 << i) != 0
                            && memory_properties.memory_types[*i as usize]
                                .property_flags
                                .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                    })
                    .context("no device-local memory for a shared image")?;
                let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(HANDLE_TYPE);
                let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(slot.image);
                slot.memory = raw.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type)
                        .push_next(&mut export)
                        .push_next(&mut dedicated),
                    None,
                )?;
                raw.bind_image_memory(slot.image, slot.memory, 0)?;
                let handle = win32.get_memory_win32_handle(
                    &vk::MemoryGetWin32HandleInfoKHR::default()
                        .memory(slot.memory)
                        .handle_type(HANDLE_TYPE),
                )?;
                (requirements.size, memory_type, handle)
            };
            if handle == 0 {
                bail!("the driver returned a null memory handle");
            }
            described.push(SlotDesc {
                handle: handle as u64,
                size: bytes,
                memory_type,
            });
            handles.push(own(handle));
            slots.push(slot);
        }
        let surface = SharedSurface::duplicate_local(&SurfaceDesc {
            generation: 1,
            width: size[0],
            height: size[1],
            format: SurfaceFormat::Rgba8Unorm,
            device: identity,
            process: std::process::id(),
            slots: described,
        })?;
        // SAFETY: `gpu` outlives the import (`Bridge::release` drops it first), and the
        // presenter's thread is the only user of its graphics queue.
        let imported = unsafe { ImportedSurface::create(gpu, &surface)? };

        // Slot 0 becomes a wgpu texture; wgpu drops the slot when it destroys it.
        let first = slots.remove(0);
        let image = first.image;
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        // SAFETY: `image` is a live RGBA8 image of `extent` on this device with
        // TRANSFER_DST usage, in UNDEFINED layout as `UNINITIALIZED` states, and its
        // memory is released by the drop callback only.
        let texture = unsafe {
            let hal_texture = hal.texture_from_raw(
                image,
                &wgpu::hal::TextureDescriptor {
                    label: Some("ara shared frame"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUses::COPY_DST,
                    memory_flags: wgpu::hal::MemoryFlags::empty(),
                    view_formats: Vec::new(),
                },
                Some(Box::new(move || drop(first))),
                wgpu::hal::vulkan::TextureMemory::External,
            );
            device.create_texture_from_hal::<wgpu::hal::api::Vulkan>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("ara shared frame"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::TextureUses::UNINITIALIZED,
            )
        };

        let mut handoff = Self {
            raw: raw.clone(),
            queue,
            family,
            pool: vk::CommandPool::null(),
            acquire: vk::CommandBuffer::null(),
            release: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            texture,
            image,
            _spare: slots,
            _handles: handles,
            _surface: surface,
            imported,
            extent,
            handed_over: false,
            in_flight: false,
        };
        // SAFETY: Live device; `Drop` destroys whatever was created if a later step fails.
        unsafe {
            handoff.pool = raw.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let buffers = raw.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(handoff.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(2),
            )?;
            (handoff.acquire, handoff.release) = (buffers[0], buffers[1]);
            handoff.fence = raw.create_fence(&vk::FenceCreateInfo::default(), None)?;
        }
        Ok(handoff)
    }

    /// Records one barrier into `command` and submits it, signalling `fence` if given.
    ///
    /// # Safety
    /// `command`'s previous submission completed; this thread is the only user of
    /// the engine's queue (the engine submits from this thread too).
    unsafe fn submit(
        &self,
        command: vk::CommandBuffer,
        barriers: &[vk::ImageMemoryBarrier<'_>],
        fence: vk::Fence,
    ) -> Result<()> {
        // SAFETY: Caller contract; the barriers name this handoff's live image.
        unsafe {
            self.raw.begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            for barrier in barriers {
                self.raw.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    std::slice::from_ref(barrier),
                );
            }
            self.raw.end_command_buffer(command)?;
            self.raw.queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&[command])],
                fence,
            )?;
        }
        Ok(())
    }

    /// Queues the copy of the engine's output into the shared slot behind the frame
    /// just rendered, and the slot's release to the presenter's device. Returns at
    /// once; [`Self::deliver`] waits.
    pub fn publish(&mut self, engine: &Engine) -> Result<()> {
        let output = engine.output().context("Renderlab has no output yet")?;
        let (src, dst) = (
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let write = vk::AccessFlags::TRANSFER_WRITE;
        if self.handed_over {
            // Take the slot back (matching the presenter's release), then discard its
            // contents into the layout wgpu tracks it in.
            // SAFETY: The previous frame's release was waited for in `deliver`, so both
            // command buffers are idle; one thread drives this queue.
            unsafe {
                self.submit(
                    self.acquire,
                    &[
                        barrier(
                            self.image,
                            (src, src),
                            (vk::QUEUE_FAMILY_EXTERNAL, self.family),
                            (vk::AccessFlags::empty(), write),
                        ),
                        barrier(
                            self.image,
                            (vk::ImageLayout::UNDEFINED, dst),
                            (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED),
                            (write, write),
                        ),
                    ],
                    vk::Fence::null(),
                )?;
            }
            self.handed_over = false;
        }
        // The output is sRGB-encoded RGBA8; the shared slot is the same bytes as UNORM.
        let mut encoder = engine
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ara shared frame copy"),
            });
        encoder.copy_texture_to_texture(
            output.as_image_copy(),
            self.texture.as_image_copy(),
            self.extent,
        );
        engine.queue().submit([encoder.finish()]);
        // SAFETY: As above; the release rides behind the copy on the same queue.
        unsafe {
            self.raw.reset_fences(&[self.fence])?;
            self.submit(
                self.release,
                &[barrier(
                    self.image,
                    (dst, src),
                    (self.family, vk::QUEUE_FAMILY_EXTERNAL),
                    (write, vk::AccessFlags::empty()),
                )],
                self.fence,
            )?;
        }
        self.in_flight = true;
        Ok(())
    }

    /// Waits for the published frame, copies it into `destination` (an RGBA8 image
    /// of the ring's size on `gpu`, in `CopySource`) and hands the slot back.
    pub fn deliver(&mut self, gpu: &GpuDevice, destination: Image) -> Result<()> {
        anyhow::ensure!(self.in_flight, "no frame was published");
        // SAFETY: The fence belongs to the release submitted in `publish`.
        unsafe { self.raw.wait_for_fences(&[self.fence], true, u64::MAX)? };
        self.in_flight = false;
        self.handed_over = true;
        // SAFETY: The release completed above, the presenter's device has not
        // acquired the slot, and `destination` is idle between presents.
        unsafe { self.imported.copy_out_image(gpu, 0, destination)? };
        Ok(())
    }
}

impl Drop for SharedHandoff {
    fn drop(&mut self) {
        // SAFETY: A release left in flight is waited for; then the pool and fence
        // have no pending work. Null handles are skipped.
        unsafe {
            if self.in_flight {
                let _ = self.raw.wait_for_fences(&[self.fence], true, u64::MAX);
            }
            if self.fence != vk::Fence::null() {
                self.raw.destroy_fence(self.fence, None);
            }
            if self.pool != vk::CommandPool::null() {
                self.raw.destroy_command_pool(self.pool, None);
            }
        }
    }
}

/// Takes ownership of an NT handle the driver returned, closing it on drop.
#[cfg(windows)]
fn own(handle: vk::HANDLE) -> Handle {
    use std::os::windows::io::FromRawHandle;
    // SAFETY: vkGetMemoryWin32HandleKHR returns a new NT handle the caller owns.
    unsafe { Handle::from_raw_handle(handle as std::os::windows::io::RawHandle) }
}
#[cfg(not(windows))]
fn own(_: vk::HANDLE) -> Handle {
    Handle
}
