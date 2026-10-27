//! Vulkan presentation: an SDL Vulkan surface and swapchain, blitting completed
//! engine images. The native `new`/`display`/`destroy` API takes the Vulkan context
//! and raw images (the editor UI); the backend-neutral methods at the end take
//! `GpuDevice` and image handles, matching the Metal `Present`.
use crate::output::select_format;
use crate::window::{Window, error};
use anyhow::{Context, Result, ensure};
use ara_color::{OutputMode, OutputSettings};
use ara_gpu::vk::Handle;
use ara_gpu::{
    GpuDevice, Image, vk,
    vulkan::{Context as Gpu, image_barrier},
};
use sdl3_sys::vulkan::SDL_Vulkan_CreateSurface;
/// Presentations that may be in flight at once (`Present::set_frames_in_flight`).
const SLOTS: usize = 2;
/// Longest a presentation waits to acquire an image or reuse a slot before it skips
/// the frame: a stalled presentation engine (minimized, occluded or undisplayed
/// window) must never block the caller's event pump.
const PRESENT_WAIT_NS: u64 = 100_000_000;
/// One presentation's acquire semaphore, and its blit and the fence that retires it.
struct Slot {
    acquire: vk::Semaphore,
    fence: vk::Fence,
    /// The submitted blit, freed once `fence` signals.
    command: Option<vk::CommandBuffer>,
}
/// Swapchain, surface and per-image presentation semaphore ownership.
pub struct Present {
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    loader: ash::khr::swapchain::Device,
    slots: [Slot; SLOTS],
    /// The slot the next presentation uses.
    next: usize,
    /// Presentations return without idling the queue (`set_frames_in_flight`).
    frames_in_flight: bool,
    rendered: Vec<vk::Semaphore>,
    images: Vec<vk::Image>,
    initialized: Vec<bool>,
    extent: vk::Extent2D,
    surface_loader: ash::khr::surface::Instance,
    output: OutputSettings,
    format: vk::SurfaceFormatKHR,
}

impl Present {
    /// Native identity for integration diagnostics. It remains stable across a
    /// loading-to-game handoff; only resize/output-mode changes replace it.
    pub fn native_swapchain(&self) -> vk::SwapchainKHR {
        self.swapchain
    }
    /// Create a swapchain matching the current pixel extent.
    /// # Safety
    /// Keep the native device and window alive until idle presentation teardown.
    pub unsafe fn new(gpu: &Gpu, window: &Window) -> Result<Self> {
        // SAFETY: The caller retains the same native owners through teardown.
        unsafe { Self::new_with_output(gpu, window, OutputSettings::default()) }
    }
    /// Creates an explicitly requested SDR, HDR10 or scRGB swapchain. HDR is
    /// rejected when unavailable; changing modes requires swapchain recreation.
    /// # Safety
    /// Keep the native device and window alive until idle presentation teardown.
    pub unsafe fn new_with_output(
        gpu: &Gpu,
        window: &Window,
        output: OutputSettings,
    ) -> Result<Self> {
        ensure!(output.valid(), "Invalid HDR paper-white/peak luminance");
        // SAFETY: Instance and SDL window remain live; all swapchain operations are
        // serialized on the graphics queue. Surface format/usage are queried.
        unsafe {
            let mut surface = std::ptr::null_mut();
            ensure!(
                SDL_Vulkan_CreateSurface(
                    window.handle,
                    gpu.instance.handle().as_raw() as usize as *mut _,
                    std::ptr::null(),
                    &mut surface
                ),
                "SDL surface: {}",
                error()
            );
            // Both dispatchable/non-dispatchable SDL Vulkan handles use pointer
            // representation on this 64-bit Windows target; ash wraps their bits.
            let surface = vk::SurfaceKHR::from_raw(surface as u64);
            let mut guard = Construction {
                gpu,
                surface,
                swapchain: vk::SwapchainKHR::null(),
                semaphores: Vec::new(),
                fences: Vec::new(),
                armed: true,
            };
            let surface_loader = ash::khr::surface::Instance::load(&gpu._entry, &gpu.instance);
            ensure!(
                surface_loader.get_physical_device_surface_support(
                    gpu.physical,
                    gpu.info.graphics_family,
                    surface
                )?,
                "Graphics queue cannot present"
            );
            let caps =
                surface_loader.get_physical_device_surface_capabilities(gpu.physical, surface)?;
            ensure!(
                caps.supported_usage_flags
                    .contains(vk::ImageUsageFlags::TRANSFER_DST),
                "Swapchain must support transfer destination"
            );
            let formats =
                surface_loader.get_physical_device_surface_formats(gpu.physical, surface)?;
            let format = select_format(&formats, output.mode)
                .with_context(|| format!("Requested {:?} output unavailable; enable Windows HDR and use an HDR display. Advertised formats: {formats:?}", output.mode))?;
            let extent = if caps.current_extent.width == u32::MAX {
                vk::Extent2D {
                    width: window.size()?[0],
                    height: window.size()?[1],
                }
            } else {
                caps.current_extent
            };
            let image_count = if caps.max_image_count == 0 {
                caps.min_image_count + 1
            } else {
                (caps.min_image_count + 1).min(caps.max_image_count)
            };
            let alpha = [
                vk::CompositeAlphaFlagsKHR::OPAQUE,
                vk::CompositeAlphaFlagsKHR::INHERIT,
                vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
                vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            ]
            .into_iter()
            .find(|a| caps.supported_composite_alpha.contains(*a))
            .context("No surface composite alpha")?;
            let loader = ash::khr::swapchain::Device::load(&gpu.instance, &gpu.device);
            let swapchain = loader.create_swapchain(
                &vk::SwapchainCreateInfoKHR::default()
                    .surface(surface)
                    .min_image_count(image_count)
                    .image_format(format.format)
                    .image_color_space(format.color_space)
                    .image_extent(extent)
                    .image_array_layers(1)
                    .image_usage(vk::ImageUsageFlags::TRANSFER_DST)
                    .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .pre_transform(caps.current_transform)
                    .composite_alpha(alpha)
                    .present_mode(vk::PresentModeKHR::FIFO)
                    .clipped(true),
                None,
            )?;
            guard.swapchain = swapchain;
            if output.mode == OutputMode::Hdr10 && gpu.info.hdr_metadata {
                let metadata = vk::HdrMetadataEXT::default()
                    .display_primary_red(vk::XYColorEXT { x: 0.708, y: 0.292 })
                    .display_primary_green(vk::XYColorEXT { x: 0.170, y: 0.797 })
                    .display_primary_blue(vk::XYColorEXT { x: 0.131, y: 0.046 })
                    .white_point(vk::XYColorEXT {
                        x: 0.3127,
                        y: 0.3290,
                    })
                    .max_luminance(output.peak_nits)
                    .min_luminance(0.001)
                    .max_content_light_level(output.peak_nits)
                    .max_frame_average_light_level(output.paper_white);
                ash::ext::hdr_metadata::Device::load(&gpu.instance, &gpu.device)
                    .set_hdr_metadata(&[swapchain], &[metadata]);
            }
            let images = loader.get_swapchain_images(swapchain)?;
            let mut slot = || -> Result<Slot> {
                let acquire = gpu
                    .device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
                guard.semaphores.push(acquire);
                let fence = gpu
                    .device
                    .create_fence(&vk::FenceCreateInfo::default(), None)?;
                guard.fences.push(fence);
                Ok(Slot {
                    acquire,
                    fence,
                    command: None,
                })
            };
            let slots = [slot()?, slot()?];
            let rendered = (0..images.len())
                .map(|_| {
                    let semaphore = gpu
                        .device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
                    guard.semaphores.push(semaphore);
                    Ok::<_, vk::Result>(semaphore)
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let initialized = vec![false; images.len()];
            guard.armed = false;
            Ok(Self {
                surface,
                swapchain,
                loader,
                slots,
                next: 0,
                frames_in_flight: false,
                rendered,
                images,
                initialized,
                extent,
                surface_loader,
                output,
                format,
            })
        }
    }
    /// Acquire the next image, signalling `semaphore`; OUT_OF_DATE is returned for
    /// caller recreation.
    /// None when no image is available within `PRESENT_WAIT_NS` (the frame is skipped;
    /// a timed-out acquire signals nothing, so `semaphore` stays reusable).
    fn acquire(&self, gpu: &Gpu, semaphore: vk::Semaphore) -> Result<Option<u32>> {
        ara_gpu::cpu_scope!("Platform.acquire");
        let result = gpu.measure_host_wait(|| {
            // SAFETY: The blit that last waited this semaphore completed (`retire_slot`).
            unsafe {
                self.loader.acquire_next_image(
                    self.swapchain,
                    PRESENT_WAIT_NS,
                    semaphore,
                    vk::Fence::null(),
                )
            }
        });
        match result {
            Ok((index, _)) => Ok(Some(index)),
            Err(vk::Result::TIMEOUT | vk::Result::NOT_READY) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    /// Lets presentations return once the blit is submitted instead of idling the
    /// graphics queue: each presentation waits only the blit that last used its
    /// slot, `SLOTS` presentations back. Only for hosts whose later writes to the
    /// source are GPU-ordered after the blit on this queue, and that wait the device
    /// before destroying it (the player window). Off by default; turning it off
    /// waits every slot.
    /// # Safety
    /// `gpu` created this presentation and is live.
    pub unsafe fn set_frames_in_flight(&mut self, gpu: &GpuDevice, enabled: bool) -> Result<()> {
        if !enabled {
            for slot in 0..SLOTS {
                // SAFETY: forwarded caller contract.
                unsafe { self.retire_slot(gpu.context(), slot, u64::MAX)? };
            }
        }
        self.frames_in_flight = enabled;
        Ok(())
    }
    /// Waits (at most `timeout_ns`) for the blit that last used `slot` and frees its
    /// command buffer; false while it is still pending (the slot stays busy).
    /// # Safety
    /// `gpu` created this presentation and is live.
    unsafe fn retire_slot(&mut self, gpu: &Gpu, slot: usize, timeout_ns: u64) -> Result<bool> {
        let Some(command) = self.slots[slot].command else {
            return Ok(true);
        };
        let fence = self.slots[slot].fence;
        ara_gpu::cpu_scope!("Platform.present_slot_wait");
        // SAFETY: the fence was submitted with `command` on this device; once it
        // signals, the command buffer and the slot's acquire semaphore are idle.
        unsafe {
            match gpu.measure_host_wait(|| gpu.device.wait_for_fences(&[fence], true, timeout_ns)) {
                Ok(()) => {}
                Err(vk::Result::TIMEOUT) => return Ok(false),
                Err(error) => return Err(error.into()),
            }
            gpu.device.reset_fences(&[fence])?;
            gpu.device
                .free_command_buffers(gpu.graphics_pool, &[command]);
        }
        self.slots[slot].command = None;
        Ok(true)
    }
    /// Per-image binary semaphore, reused only after image reacquisition.
    pub fn rendered(&self, index: u32) -> vk::Semaphore {
        self.rendered[index as usize]
    }
    /// Record a blit from an owned offscreen CopySource image.
    fn record(
        &mut self,
        gpu: &Gpu,
        cmd: vk::CommandBuffer,
        source: vk::Image,
        size: [u32; 2],
        index: u32,
    ) {
        let old = if self.initialized[index as usize] {
            vk::ImageLayout::PRESENT_SRC_KHR
        } else {
            vk::ImageLayout::UNDEFINED
        };
        self.initialized[index as usize] = true;
        let destination = self.images[index as usize];
        // SAFETY: Caller waits on image acquisition at ALL_COMMANDS and signals a
        // per-image presentation semaphore. Source is in TRANSFER_SRC_OPTIMAL.
        unsafe {
            let barrier = image_barrier(
                destination,
                vk::ImageAspectFlags::COLOR,
                old,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                (
                    vk::PipelineStageFlags2::BLIT,
                    vk::AccessFlags2::TRANSFER_WRITE,
                ),
            );
            gpu.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&[barrier]),
            );
            let layers = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1);
            let region = vk::ImageBlit::default()
                .src_subresource(layers)
                .dst_subresource(layers)
                .src_offsets([
                    vk::Offset3D::default(),
                    vk::Offset3D {
                        x: size[0] as i32,
                        y: size[1] as i32,
                        z: 1,
                    },
                ])
                .dst_offsets([
                    vk::Offset3D::default(),
                    vk::Offset3D {
                        x: self.extent.width as i32,
                        y: self.extent.height as i32,
                        z: 1,
                    },
                ]);
            gpu.device.cmd_blit_image(
                cmd,
                source,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                destination,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
                vk::Filter::NEAREST,
            );
            let barrier = image_barrier(
                destination,
                vk::ImageAspectFlags::COLOR,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::PRESENT_SRC_KHR,
                (
                    vk::PipelineStageFlags2::BLIT,
                    vk::AccessFlags2::TRANSFER_WRITE,
                ),
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
            );
            gpu.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&[barrier]),
            );
        }
    }
    /// Present the acquired image after its binary render semaphore.
    fn present(&self, gpu: &Gpu, index: u32) -> Result<()> {
        gpu.measure_host_wait(|| {
            // SAFETY: Submit signals this acquired image's semaphore; reacquisition
            // before reuse proves the previous present consumed it.
            unsafe {
                self.loader.queue_present(
                    gpu.graphics,
                    &vk::PresentInfoKHR::default()
                        .wait_semaphores(&[self.rendered(index)])
                        .swapchains(&[self.swapchain])
                        .image_indices(&[index]),
                )
            }
        })?;
        Ok(())
    }
    /// Destroy after device idle, before the SDL window or Vulkan instance.
    /// # Safety
    /// Pass the creating device after all its queues and presentation work are idle.
    pub unsafe fn destroy(&mut self, gpu: &Gpu) {
        // SAFETY: Called after device idle (including present), before window/instance drop.
        unsafe {
            for slot in &mut self.slots {
                if let Some(command) = slot.command.take() {
                    gpu.device
                        .free_command_buffers(gpu.graphics_pool, &[command]);
                }
                if slot.acquire != vk::Semaphore::null() {
                    gpu.device.destroy_semaphore(slot.acquire, None);
                    slot.acquire = vk::Semaphore::null();
                }
                if slot.fence != vk::Fence::null() {
                    gpu.device.destroy_fence(slot.fence, None);
                    slot.fence = vk::Fence::null();
                }
            }
            for semaphore in &self.rendered {
                gpu.device.destroy_semaphore(*semaphore, None);
            }
            self.rendered.clear();
            if self.swapchain != vk::SwapchainKHR::null() {
                self.loader.destroy_swapchain(self.swapchain, None);
                self.swapchain = vk::SwapchainKHR::null();
            }
            if self.surface != vk::SurfaceKHR::null() {
                self.surface_loader.destroy_surface(self.surface, None);
                self.surface = vk::SurfaceKHR::null();
            }
        }
    }
}
impl Present {
    /// Blit one completed Ara frame to the acquired image and wait for presentation work.
    /// The serialized v0 path keeps native command and semaphore ownership inspectable.
    /// # Safety
    /// Source must be a completed CopySource image owned by the creating device.
    /// Keep both device and SDL window alive for the whole presentation lifetime.
    pub unsafe fn display(&mut self, gpu: &Gpu, source: vk::Image, size: [u32; 2]) -> Result<()> {
        ensure!(
            self.output.mode == OutputMode::Sdr,
            "HDR requires display_encoded with matching output encoding"
        );
        // SAFETY: Same completed CopySource image and native owner contract.
        unsafe { self.display_encoded(gpu, source, size, OutputMode::Sdr) }
    }
    /// Active display transfer and paper-white policy for the output pass.
    pub fn output_settings(&self) -> OutputSettings {
        self.output
    }
    /// Actual queried native format and color space selected for this swapchain.
    pub fn surface_format(&self) -> vk::SurfaceFormatKHR {
        self.format
    }
    /// Presents an image already encoded for the selected display. PQ is stored
    /// as numerical UNORM/float values; scRGB remains linear and may exceed 1.
    /// # Safety
    /// Source must be a completed CopySource image on this device. Its format
    /// must support native blitting and preserve the selected encoding/range.
    pub unsafe fn display_encoded(
        &mut self,
        gpu: &Gpu,
        source: vk::Image,
        size: [u32; 2],
        encoding: OutputMode,
    ) -> Result<()> {
        ara_gpu::cpu_scope!("Platform.display");
        ensure!(
            encoding == self.output.mode,
            "Presentation/output encoding mismatch"
        );
        ensure!(
            self.swapchain != vk::SwapchainKHR::null(),
            "Presentation has been destroyed"
        );
        let slot = self.next;
        // A stalled presentation engine skips frames rather than blocking the caller.
        // SAFETY: this presentation was created on `gpu`.
        if !unsafe { self.retire_slot(gpu, slot, PRESENT_WAIT_NS)? } {
            return Ok(());
        }
        let Some(index) = self.acquire(gpu, self.slots[slot].acquire)? else {
            return Ok(());
        };
        let cmd = gpu.commands(false)?;
        // SAFETY: Fresh command buffer, never submitted.
        unsafe {
            gpu.begin(cmd)?;
        }
        self.record(gpu, cmd, source, size, index);
        // SAFETY: Source is complete and CopySource; the acquired image is waited at ALL_COMMANDS.
        unsafe {
            gpu.device.end_command_buffer(cmd)?;
            let commands = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
            let waits = [vk::SemaphoreSubmitInfo::default()
                .semaphore(self.slots[slot].acquire)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let signals = [vk::SemaphoreSubmitInfo::default()
                .semaphore(self.rendered(index))
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            gpu.device.queue_submit2(
                gpu.graphics,
                &[vk::SubmitInfo2::default()
                    .command_buffer_infos(&commands)
                    .wait_semaphore_infos(&waits)
                    .signal_semaphore_infos(&signals)],
                if self.frames_in_flight {
                    self.slots[slot].fence
                } else {
                    vk::Fence::null()
                },
            )?;
        }
        let result = self.present(gpu, index);
        if self.frames_in_flight {
            // The slot's next presentation waits its fence (`retire_slot`).
            self.slots[slot].command = Some(cmd);
            self.next = (slot + 1) % SLOTS;
            return result;
        }
        // SAFETY: Waiting includes the blit/present queue operations before command reuse/free.
        unsafe {
            ara_gpu::cpu_scope!("Platform.present_queue_idle");
            gpu.measure_host_wait(|| gpu.device.queue_wait_idle(gpu.graphics))?;
            gpu.device.free_command_buffers(gpu.graphics_pool, &[cmd]);
        }
        result
    }
}

struct Construction<'a> {
    gpu: &'a Gpu,
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    semaphores: Vec<vk::Semaphore>,
    fences: Vec<vk::Fence>,
    armed: bool,
}
impl Drop for Construction<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: These partially constructed objects have never been submitted or presented.
        unsafe {
            for semaphore in &self.semaphores {
                self.gpu.device.destroy_semaphore(*semaphore, None);
            }
            for fence in &self.fences {
                self.gpu.device.destroy_fence(*fence, None);
            }
            if self.swapchain != vk::SwapchainKHR::null() {
                ash::khr::swapchain::Device::load(&self.gpu.instance, &self.gpu.device)
                    .destroy_swapchain(self.swapchain, None);
            }
            ash::khr::surface::Instance::load(&self.gpu._entry, &self.gpu.instance)
                .destroy_surface(self.surface, None);
        }
    }
}

/// The backend-neutral surface shared with the Metal `Present`.
impl Present {
    /// [`Self::new`] on a backend-neutral device.
    /// # Safety
    /// Keep the device and window alive until [`Self::retire`].
    pub unsafe fn create(gpu: &GpuDevice, window: &Window) -> Result<Self> {
        // SAFETY: forwarded caller contract.
        unsafe { Self::new(gpu.context(), window) }
    }
    /// [`Self::new_with_output`] on a backend-neutral device.
    /// # Safety
    /// As [`Self::create`].
    pub unsafe fn create_with_output(
        gpu: &GpuDevice,
        window: &Window,
        output: OutputSettings,
    ) -> Result<Self> {
        // SAFETY: forwarded caller contract.
        unsafe { Self::new_with_output(gpu.context(), window, output) }
    }
    /// Metal's presentation copies run on a queue of their own, so the next frame's
    /// work waits for them there; this blit shares the renderer's queue already.
    pub fn order_before(&self, _gpu: &mut GpuDevice) {}
    /// Blit a completed engine image of `size` ([`Self::display`]).
    /// # Safety
    /// `image` is a completed CopySource image on `gpu`; both owners are live.
    pub unsafe fn present_image(
        &mut self,
        gpu: &GpuDevice,
        image: Image,
        size: [u32; 2],
    ) -> Result<()> {
        let source = gpu.native_image(image)?;
        // SAFETY: forwarded caller contract for the native image of `image`.
        unsafe { self.display(gpu.context(), source, size) }
    }
    /// Present an image encoded for the selected display ([`Self::display_encoded`]).
    /// # Safety
    /// As [`Self::present_image`], with a source format that preserves `encoding`.
    pub unsafe fn present_encoded(
        &mut self,
        gpu: &GpuDevice,
        image: Image,
        size: [u32; 2],
        encoding: OutputMode,
    ) -> Result<()> {
        let source = gpu.native_image(image)?;
        // SAFETY: forwarded caller contract for the native image of `image`.
        unsafe { self.display_encoded(gpu.context(), source, size, encoding) }
    }
    /// Destroy the swapchain and surface ([`Self::destroy`]); callers that need a
    /// device-wide idle wait (resize, shutdown) wait first.
    /// # Safety
    /// Created on `gpu`; no presentation work is in flight and it is not used again.
    pub unsafe fn retire(&mut self, gpu: &GpuDevice) {
        // SAFETY: forwarded caller contract.
        unsafe { self.destroy(gpu.context()) }
    }
}
