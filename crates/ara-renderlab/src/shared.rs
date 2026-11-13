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
//! A second ring, the same size and format, carries the object-id image. It is
//! copied and released in the same submissions as the colour slot, on one fence, so
//! the presenter never reads ids from a different frame than the colour it copies.
//! [`SharedHandoff::set_id`] is the writer's entry: pass Renderlab's id texture
//! (RGBA8 UNORM, `COPY_SRC`, the view size) and [`SharedHandoff::publish`] copies it
//! beside the colour output. [`SharedHandoff::deliver_id`] copies that image out,
//! and it must run before the next publish.
//!
//! Slot 0 of each ring is also a wgpu texture, so the frame is copied into it by
//! wgpu (which tracks it in `COPY_DST`, that is `TRANSFER_DST_OPTIMAL`); the
//! ownership barriers around that copy are raw Vulkan on the same queue and leave
//! the image in the layout wgpu expects.
use anyhow::{Context as _, Result, bail};
use ara_gpu::{
    Access, Barrier, Device, GpuDevice, Image, Queue, Resource,
    shared::{DeviceIdentity, SLOTS, SharedSurface, SlotDesc, SurfaceDesc, SurfaceFormat},
    vulkan::external::ImportedSurface,
};
use ash038::{self as ash, vk};
use renderlab::{ViewId, engine::Engine};

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

/// One exported ring: colour or ids. The import drops before the texture, so the
/// presenter's view of the memory is gone before the export frees it.
struct SharedImage {
    imported: ImportedSurface,
    /// Slot 0 as wgpu sees it; wgpu destroys that slot when it destroys the texture.
    texture: wgpu::Texture,
    image: vk::Image,
    /// The ring's other slots: imported, never written.
    _spare: Vec<Slot>,
    _handles: Vec<Handle>,
    _surface: SharedSurface,
}

/// The shared rings on Renderlab's device and their imports on the presenter's.
pub(crate) struct SharedHandoff {
    raw: ash::Device,
    queue: vk::Queue,
    family: u32,
    pool: vk::CommandPool,
    /// Recorded per frame: acquire before the copies, release after.
    acquire: vk::CommandBuffer,
    release: vk::CommandBuffer,
    fence: vk::Fence,
    colour: SharedImage,
    /// Object ids for the same frame as [`Self::colour`].
    id: SharedImage,
    /// Copied beside the colour output when set. See [`Self::set_id`].
    id_source: Option<wgpu::Texture>,
    extent: wgpu::Extent3d,
    /// Colour slot 0 rests with the presenter's side: the next frame acquires it first.
    handed_over: bool,
    /// Id slot 0 rests with the presenter's side.
    id_handed_over: bool,
    /// A release was submitted and not yet waited for.
    in_flight: bool,
    /// The synthetic id texture [`Self::stage_pattern`] rewrites, when that switch is on.
    synthetic: Option<wgpu::Texture>,
    /// Frame number in the synthetic pattern's alpha, 1 to 255.
    pattern_frame: u32,
    /// The release fence was waited; a copy-out may still be owed.
    frame_latched: bool,
    /// The waited frame's colour has not been copied out.
    colour_owed: bool,
    /// The waited frame's id has not been copied out. False when that frame had no id.
    id_owed: bool,
}

/// What both rings are exported from.
#[derive(Clone, Copy)]
struct RingExport<'a> {
    device: &'a wgpu::Device,
    raw: &'a ash::Device,
    win32: &'a ash::khr::external_memory_win32::Device,
    memory: &'a vk::PhysicalDeviceMemoryProperties,
    identity: DeviceIdentity,
    gpu: &'a GpuDevice,
    size: [u32; 2],
    extent: wgpu::Extent3d,
}

/// One [`SLOTS`]-image ring, imported on `gpu`, with slot 0 wrapped for wgpu copies.
fn export_ring(
    ctx: &RingExport<'_>,
    hal: &impl std::ops::Deref<Target = wgpu::hal::vulkan::Device>,
    label: &'static str,
) -> Result<SharedImage> {
    let RingExport {
        device,
        raw,
        win32,
        memory: memory_properties,
        identity,
        gpu,
        size,
        extent,
    } = *ctx;
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
    // SAFETY: `gpu` outlives the import (`Bridge::release` drops the handoff first),
    // and the presenter's thread is the only user of its graphics queue.
    let imported = unsafe { ImportedSurface::create(gpu, &surface)? };

    // Slot 0 becomes a wgpu texture; wgpu drops the slot when it destroys it.
    let first = slots.remove(0);
    let image = first.image;
    // SAFETY: `image` is a live RGBA8 image of `extent` on this device with
    // TRANSFER_DST usage, in UNDEFINED layout as `UNINITIALIZED` states, and its
    // memory is released by the drop callback only.
    let texture = unsafe {
        let hal_texture = hal.texture_from_raw(
            image,
            &wgpu::hal::TextureDescriptor {
                label: Some(label),
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
                label: Some(label),
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
    Ok(SharedImage {
        imported,
        texture,
        image,
        _spare: slots,
        _handles: handles,
        _surface: surface,
    })
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
    /// Exports both rings of `size` on the engine's device and imports them on `gpu`.
    /// Fails (the caller falls back to the CPU copy) off Windows, when the two
    /// devices are different adapters, or when the driver cannot share RGBA8 images.
    pub fn new(engine: &Engine, gpu: &GpuDevice, size: [u32; 2]) -> Result<Self> {
        Self::on_device(engine.device(), gpu, size)
    }

    /// Exports both rings on `device` and imports them on `gpu`. `device` is
    /// Renderlab's, or another wgpu device on the same adapter (the id-slot test).
    pub(crate) fn on_device(
        device: &wgpu::Device,
        gpu: &GpuDevice,
        size: [u32; 2],
    ) -> Result<Self> {
        if !cfg!(windows) {
            bail!("shared images use opaque NT handles (Windows only)");
        }
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
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let export = RingExport {
            device,
            raw: &raw,
            win32: &win32,
            memory: &memory_properties,
            identity,
            gpu,
            size,
            extent,
        };
        let colour = export_ring(&export, &hal, "ara shared frame")?;
        let id = export_ring(&export, &hal, "ara shared id")?;

        let mut handoff = Self {
            raw: raw.clone(),
            queue,
            family,
            pool: vk::CommandPool::null(),
            acquire: vk::CommandBuffer::null(),
            release: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            colour,
            id,
            id_source: None,
            extent,
            handed_over: false,
            id_handed_over: false,
            in_flight: false,
            frame_latched: false,
            colour_owed: false,
            id_owed: false,
            synthetic: None,
            pattern_frame: 0,
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

    /// The id image [`Self::publish`] copies beside the colour frame, and each
    /// publish after until replaced. `id` is `Rgba8Unorm` (not sRGB) at this
    /// handoff's size, with `COPY_SRC`.
    ///
    /// Renderlab's id texture replaces the synthetic writer by this call alone:
    /// `handoff.set_id(id_texture)`, then the existing `publish` / `deliver` pair
    /// plus [`Self::deliver_id`] before the next publish.
    ///
    /// The bridge does not call this until that texture exists. The id-slot test
    /// does, and a `cfg(test)` caller is invisible to the library build.
    #[allow(dead_code)]
    pub fn set_id(&mut self, id: wgpu::Texture) {
        self.id_source = Some(id);
    }

    /// Queues the copy of the view's output into the shared slot behind the frame
    /// just rendered, and the slot's release to the presenter's device. When
    /// [`Self::set_id`] was called, that texture is copied and released in the same
    /// submissions, on the same fence. Returns at once; [`Self::deliver`] waits.
    pub fn publish(&mut self, engine: &Engine, view: ViewId) -> Result<()> {
        let output = engine
            .view_output(view)
            .context("Renderlab has no output yet")?;
        self.copy_frame(engine.device(), engine.queue(), output)
    }

    /// Whether the ring holds no published frame: nothing is in flight and every copy-out is
    /// done, so [`Self::publish`] may run.
    pub fn idle(&self) -> bool {
        !self.in_flight && !self.colour_owed && !self.id_owed
    }

    /// Whether this frame's id image is still owed to [`Self::deliver_id`].
    #[allow(dead_code)]
    pub fn id_pending(&self) -> bool {
        self.id_owed
    }

    /// Writes the id-slot test pattern for the next frame (alpha is the frame
    /// number, 1 to 255) into an RGBA8 UNORM texture and [`Self::set_id`]s it.
    ///
    /// `ARA_RENDERLAB_SCENE_IDS=pattern` is the debug switch the bridge checks
    /// before calling this, until Renderlab's real id texture replaces it.
    #[allow(dead_code)]
    pub fn stage_pattern(&mut self, engine: &Engine) -> Result<()> {
        self.pattern_frame = self.pattern_frame % 255 + 1;
        if self.synthetic.is_none() {
            self.synthetic = Some(pattern_texture(engine.device(), self.extent));
        }
        let texture = self.synthetic.clone().context("pattern texture")?;
        let bytes = id_pattern([self.extent.width, self.extent.height], self.pattern_frame);
        write_rgba(engine.queue(), &texture, &bytes);
        self.set_id(texture);
        Ok(())
    }

    /// [`Self::publish`] with an explicit colour texture. The id slot still comes
    /// from [`Self::set_id`]. Tests pass a synthetic colour image; production passes
    /// `Engine::output` through [`Self::publish`].
    #[allow(dead_code)]
    pub(crate) fn publish_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        colour: &wgpu::Texture,
    ) -> Result<()> {
        self.copy_frame(device, queue, colour)
    }

    /// The copy is the whole image; a different size would read off the slot.
    fn expect_size(&self, texture: &wgpu::Texture, what: &str) -> Result<()> {
        let got = texture.size();
        anyhow::ensure!(
            got.width == self.extent.width && got.height == self.extent.height,
            "{what} is {}x{}, the shared slot is {}x{}",
            got.width,
            got.height,
            self.extent.width,
            self.extent.height
        );
        Ok(())
    }

    fn copy_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        colour: &wgpu::Texture,
    ) -> Result<()> {
        if self.in_flight || self.colour_owed || self.id_owed {
            bail!("the previous shared frame was not delivered");
        }
        self.expect_size(colour, "the colour image")?;
        if let Some(id) = &self.id_source {
            self.expect_size(id, "the id image")?;
            anyhow::ensure!(
                id.format() == wgpu::TextureFormat::Rgba8Unorm,
                "the id image must be RGBA8 UNORM so the copy stays bit-exact, got {:?}",
                id.format()
            );
        }
        let id = self.id_source.clone();
        let mut acquire = Vec::new();
        if self.handed_over {
            // Take the slot back (matching the presenter's release), then discard its
            // contents into the layout wgpu tracks it in.
            acquire.extend(reclaim(self.colour.image, self.family));
        }
        if self.id_handed_over {
            acquire.extend(reclaim(self.id.image, self.family));
        }
        if !acquire.is_empty() {
            // SAFETY: The previous frame's release was waited for in `deliver`, so both
            // command buffers are idle; one thread drives this queue.
            unsafe { self.submit(self.acquire, &acquire, vk::Fence::null())? };
            self.handed_over = false;
            self.id_handed_over = false;
        }
        // Colour bytes are the engine's sRGB-encoded RGBA8 stored as UNORM. The id
        // image is already UNORM bytes. Neither copy converts.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ara shared frame copy"),
        });
        encoder.copy_texture_to_texture(
            colour.as_image_copy(),
            self.colour.texture.as_image_copy(),
            self.extent,
        );
        if let Some(id) = &id {
            encoder.copy_texture_to_texture(
                id.as_image_copy(),
                self.id.texture.as_image_copy(),
                self.extent,
            );
        }
        queue.submit([encoder.finish()]);
        let mut release = vec![release_to(self.colour.image, self.family)];
        if id.is_some() {
            release.push(release_to(self.id.image, self.family));
        }
        // SAFETY: As above; the release rides behind the copies on the same queue.
        unsafe {
            self.raw.reset_fences(&[self.fence])?;
            self.submit(self.release, &release, self.fence)?;
        }
        self.in_flight = true;
        self.frame_latched = false;
        self.colour_owed = true;
        self.id_owed = id.is_some();
        Ok(())
    }

    /// Waits for the published frame once, so colour and id can be copied out separately.
    fn wait_frame(&mut self) -> Result<()> {
        if self.in_flight {
            // SAFETY: The fence belongs to the release submitted in `publish`.
            unsafe { self.raw.wait_for_fences(&[self.fence], true, u64::MAX)? };
            self.in_flight = false;
            self.frame_latched = true;
        }
        anyhow::ensure!(self.frame_latched, "no frame was published");
        Ok(())
    }

    fn settle_latch(&mut self) {
        if !self.colour_owed && !self.id_owed {
            self.frame_latched = false;
        }
    }

    /// Waits for the published frame, copies it into `destination` (an RGBA8 image
    /// of the ring's size on `gpu`, in `CopySource`) and hands the slot back.
    pub fn deliver(&mut self, gpu: &GpuDevice, destination: Image) -> Result<()> {
        self.wait_frame()?;
        anyhow::ensure!(self.colour_owed, "no frame was published");
        self.colour_owed = false;
        self.handed_over = true;
        // SAFETY: The release completed above, the presenter's device has not
        // acquired the slot, and `destination` is idle between presents.
        unsafe { self.colour.imported.copy_out_image(gpu, 0, destination)? };
        self.settle_latch();
        Ok(())
    }

    /// Copies the id image of the frame [`Self::deliver`] copies, into `destination`
    /// (RGBA8 UNORM of the ring's size on `gpu`, in `CopySource`), and hands that
    /// slot back. Call it before the next [`Self::publish`] once [`Self::set_id`]
    /// is in use. Either copy-out may run first; both wait on the same fence.
    #[allow(dead_code)]
    pub fn deliver_id(&mut self, gpu: &GpuDevice, destination: Image) -> Result<()> {
        self.wait_frame()?;
        anyhow::ensure!(self.id_owed, "this frame published no id image");
        self.id_owed = false;
        self.id_handed_over = true;
        // SAFETY: The same release as `deliver` completed, this device has not
        // acquired the id slot, and `destination` is idle.
        unsafe { self.id.imported.copy_out_image(gpu, 0, destination)? };
        self.settle_latch();
        Ok(())
    }
}

/// Acquire a slot from the presenter and discard it into `TRANSFER_DST`, which is
/// the layout wgpu tracks the texture in.
fn reclaim(image: vk::Image, family: u32) -> [vk::ImageMemoryBarrier<'static>; 2] {
    let src = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
    let dst = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
    let write = vk::AccessFlags::TRANSFER_WRITE;
    [
        barrier(
            image,
            (src, src),
            (vk::QUEUE_FAMILY_EXTERNAL, family),
            (vk::AccessFlags::empty(), write),
        ),
        barrier(
            image,
            (vk::ImageLayout::UNDEFINED, dst),
            (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED),
            (write, write),
        ),
    ]
}

/// Release a slot to the presenter: `TRANSFER_DST` to `TRANSFER_SRC`, owned by external.
fn release_to(image: vk::Image, family: u32) -> vk::ImageMemoryBarrier<'static> {
    let src = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
    let dst = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
    let write = vk::AccessFlags::TRANSFER_WRITE;
    barrier(
        image,
        (dst, src),
        (family, vk::QUEUE_FAMILY_EXTERNAL),
        (write, vk::AccessFlags::empty()),
    )
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

/// `ARA_RENDERLAB_SCENE_IDS=pattern` asks the bridge to stage the synthetic id image.
#[allow(dead_code)]
pub const SCENE_IDS_PATTERN: &str = "pattern";

/// Whether the debug switch is the synthetic packed-id pattern.
#[allow(dead_code)]
pub fn scene_ids_pattern() -> bool {
    std::env::var("ARA_RENDERLAB_SCENE_IDS").ok().as_deref() == Some(SCENE_IDS_PATTERN)
}

/// Packed id bytes for one frame: texel (0, 0) is 0, texel (1, 0) is `0xFFFF_FFFF`,
/// and every other texel's alpha is `frame` (1 to 255).
pub(crate) fn id_pattern(size: [u32; 2], frame: u32) -> Vec<u8> {
    let (width, height) = (size[0], size[1]);
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            bytes.extend(id_texel(x, y, width, frame));
        }
    }
    bytes
}

fn id_texel(x: u32, y: u32, width: u32, frame: u32) -> [u8; 4] {
    if x == 0 && y == 0 {
        return [0; 4];
    }
    if x == 1 && y == 0 {
        return [0xff; 4];
    }
    let n = x + y * width;
    [
        n.wrapping_mul(17).wrapping_add(3) as u8,
        n.wrapping_mul(29).wrapping_add(5) as u8,
        n.wrapping_mul(41).wrapping_add(7) as u8,
        frame as u8,
    ]
}

fn pattern_texture(device: &wgpu::Device, extent: wgpu::Extent3d) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ara synthetic id"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_rgba(queue: &wgpu::Queue, texture: &wgpu::Texture, tight: &[u8]) {
    let size = texture.size();
    let row = size.width * 4;
    let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let mut padded = vec![0u8; (stride * size.height) as usize];
    for y in 0..size.height as usize {
        let from = y * row as usize;
        let to = y * stride as usize;
        padded[to..to + row as usize].copy_from_slice(&tight[from..from + row as usize]);
    }
    queue.write_texture(
        texture.as_image_copy(),
        &padded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(size.height),
        },
        size,
    );
}

/// The id image retained on the compositor's device: RGBA8 UNORM, `CopySource`,
/// sampled with an exact texel fetch (`IdSource::Packed`).
pub struct PresentedId {
    image: Image,
    size: [u32; 2],
}

#[allow(dead_code)]
impl PresentedId {
    /// The retained image.
    pub fn output(&self) -> Image {
        self.image
    }

    /// Copies this frame's id out of `handoff` into a retained image of `size`.
    /// Call it after [`SharedHandoff::deliver`] and before the next publish.
    #[allow(dead_code)]
    pub fn copy_from(
        slot: &mut Option<Self>,
        handoff: &mut SharedHandoff,
        gpu: &mut GpuDevice,
        size: [u32; 2],
    ) -> Result<Image> {
        if slot.as_ref().is_none_or(|kept| kept.size != size) {
            if let Some(old) = slot.take() {
                old.destroy(gpu)?;
            }
            let image = gpu.create_image(
                size[0],
                size[1],
                ara_gpu::vk::Format::R8G8B8A8_UNORM,
                ara_gpu::vk::ImageUsageFlags::TRANSFER_SRC
                    | ara_gpu::vk::ImageUsageFlags::TRANSFER_DST
                    | ara_gpu::vk::ImageUsageFlags::SAMPLED,
            )?;
            settle_copy_source(gpu, image)?;
            *slot = Some(Self { image, size });
        }
        let Some(kept) = slot.as_ref() else {
            bail!("the id image was not retained");
        };
        let image = kept.image;
        handoff.deliver_id(gpu, image)?;
        Ok(image)
    }

    /// Releases the retained image before its device is destroyed.
    pub fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.image))?;
        gpu.collect()?;
        Ok(())
    }
}

fn settle_copy_source(gpu: &mut GpuDevice, image: Image) -> Result<()> {
    let mut commands = gpu.begin(Queue::Graphics);
    for (before, after) in [
        (Access::Undefined, Access::CopyDestination),
        (Access::CopyDestination, Access::CopySource),
    ] {
        commands.barriers([Barrier {
            resource: Resource::Image(image),
            before,
            after,
            handoff: None,
        }]);
    }
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.wait(point)?;
    Ok(())
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
