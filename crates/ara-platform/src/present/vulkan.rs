//! Vulkan presentation: SDL or native HWND surfaces and swapchains, blitting completed
//! engine images. The native `new`/`display`/`destroy` API takes the Vulkan context
//! and raw images (the editor UI); the backend-neutral methods at the end take
//! `GpuDevice` and image handles, matching the Metal `Present`.
use crate::output::select_format;
use crate::window::{Window, error};
use anyhow::{Context, Result, ensure};
use ara_color::{OutputMode, OutputSettings};
use ara_gpu::vk::{Handle, TaggedStructure};
use ara_gpu::{
    GpuDevice, Image, vk,
    vulkan::{Context as Gpu, image_barrier},
};
use sdl3_sys::vulkan::SDL_Vulkan_CreateSurface;
use std::time::{Duration, Instant};
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
    /// The min and max image extents this surface presents 1:1 at the top left
    /// (`VK_EXT_swapchain_maintenance1`), when supported: a swapchain of another
    /// size is then cropped or padded, never stretched.
    scaling: Option<[vk::Extent2D; 2]>,
    /// This swapchain is a live-resize one (`resize` with a reach): larger than the
    /// window and presented 1:1. Others match the window and stretch the source.
    oversized: bool,
    /// Present mode wish (`set_vsync`) and the one the current swapchain was built with.
    vsync: bool,
    built_vsync: bool,
    /// Native-layer size, also used to recover an out-of-date child swapchain.
    native_size: Option<[u32; 2]>,
    pending_size: Option<[u32; 2]>,
    wait_ns: u64,
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
        let size = window.size()?;
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
            Self::from_surface(gpu, surface, size, output)
        }
    }
    /// Create an SDR presentation for a live child HWND without SDL ownership.
    /// Acquiring or reusing an image waits at most one millisecond per wait.
    /// # Safety
    /// `hwnd` must belong to this thread and outlive the presentation. `gpu` must
    /// enable VK_KHR_surface and VK_KHR_win32_surface and outlive teardown.
    #[cfg(windows)]
    pub unsafe fn from_hwnd(
        gpu: &Gpu,
        hwnd: std::num::NonZeroIsize,
        size: [u32; 2],
    ) -> Result<Self> {
        ensure!(size.into_iter().all(|v| v > 0), "Empty presentation size");
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        }
        // SAFETY: the caller retains this live HWND on its owner thread. -6 is
        // GWLP_HINSTANCE, the module that registered the window's class.
        let instance = unsafe { GetWindowLongPtrW(hwnd.get(), -6) };
        ensure!(instance != 0, "HWND has no owning module");
        let loader = ash::khr::win32_surface::Instance::load(&gpu._entry, &gpu.instance);
        // SAFETY: live instance/window/module, retained by the caller. Construction
        // takes ownership of the surface even if swapchain creation fails.
        unsafe {
            let surface = loader.create_win32_surface(
                &vk::Win32SurfaceCreateInfoKHR::default()
                    .hinstance(instance)
                    .hwnd(hwnd.get()),
                None,
            )?;
            let mut present = Self::from_surface(gpu, surface, size, OutputSettings::default())?;
            present.native_size = Some(size);
            present.wait_ns = 1_000_000;
            Ok(present)
        }
    }
    /// Takes ownership of a newly created surface, including on construction failure.
    /// # Safety
    /// The surface belongs to `gpu` and its native window remains live.
    unsafe fn from_surface(
        gpu: &Gpu,
        surface: vk::SurfaceKHR,
        size: [u32; 2],
        output: OutputSettings,
    ) -> Result<Self> {
        // SAFETY: forwarded native-owner contract; the guard cleans partial resources.
        unsafe {
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
            let loader = ash::khr::swapchain::Device::load(&gpu.instance, &gpu.device);
            let scaling = one_to_one(gpu, surface)?;
            let shape = Shape {
                output,
                scaling,
                request: None,
                vsync: true,
            };
            let (swapchain, extent, format) = build_swapchain(
                gpu,
                &loader,
                &surface_loader,
                surface,
                size,
                vk::SwapchainKHR::null(),
                &shape,
            )?;
            guard.swapchain = swapchain;
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
                scaling,
                oversized: false,
                vsync: true,
                built_vsync: true,
                native_size: None,
                pending_size: None,
                wait_ns: PRESENT_WAIT_NS,
            })
        }
    }
    /// Match the swapchain to the window's current size, in place: the surface, slots
    /// and semaphores stay and the old swapchain hands over through `oldSwapchain`.
    /// Waits only this presentation's blits and the graphics queue (which retires
    /// presents waiting on the per-image semaphores), never the whole device.
    ///
    /// Creating a swapchain costs the driver ~8 ms (NVIDIA, Windows), too much for
    /// every frame of a live window resize. With `reach` (the largest size the
    /// resize can reach, in pixels; the window may still be smaller or larger) and
    /// present scaling, a swapchain that covers it is kept and presented 1:1 at the
    /// top left, cropped to the window; one that does not is recreated at `reach`.
    /// Without `reach`, or without present scaling, it is recreated at exactly the
    /// window's size. False (nothing changed) while a blit is still pending.
    /// # Safety
    /// `gpu` created this presentation; this thread owns its graphics queue, and
    /// `window` is the one it was created for.
    pub unsafe fn resize(
        &mut self,
        gpu: &Gpu,
        window: &Window,
        reach: Option<[u32; 2]>,
    ) -> Result<bool> {
        ara_gpu::cpu_scope!("Platform.resize");
        // SAFETY: forwarded native-owner contract.
        unsafe { self.resize_extent(gpu, window.size()?, reach) }
    }
    /// Recreate a native presentation at an explicit pixel extent. False retries
    /// later when the preceding blit has not retired within its bounded wait.
    /// # Safety
    /// `gpu` created this presentation; the native window and device remain live,
    /// and this thread serializes graphics-queue access.
    pub unsafe fn resize_to(&mut self, gpu: &Gpu, size: [u32; 2]) -> Result<bool> {
        ensure!(size.into_iter().all(|v| v > 0), "Empty presentation size");
        // SAFETY: forwarded native-owner contract.
        let resized = unsafe { self.resize_extent(gpu, size, None)? };
        if resized {
            self.pending_size = None;
            if self.native_size.is_some() {
                self.native_size = Some(size);
            }
        }
        Ok(resized)
    }
    /// Queue a native-layer resize for the next display call, which has the GPU
    /// context. Repeated requests coalesce; no raw device pointer is retained.
    #[cfg(windows)]
    pub fn request_resize(&mut self, size: [u32; 2]) -> Result<()> {
        ensure!(self.native_size.is_some(), "Not a native HWND presentation");
        ensure!(size.into_iter().all(|v| v > 0), "Empty presentation size");
        self.pending_size = Some(size);
        Ok(())
    }
    /// Shared explicit-size recreation for SDL and native layers.
    /// # Safety
    /// Same live native owners and exclusive queue access as `resize_to`.
    unsafe fn resize_extent(
        &mut self,
        gpu: &Gpu,
        size: [u32; 2],
        reach: Option<[u32; 2]>,
    ) -> Result<bool> {
        let reach = reach
            .filter(|_| self.scaling.is_some())
            .map(|r| [r[0].max(size[0]), r[1].max(size[1])]);
        if self.oversized
            && self.vsync == self.built_vsync
            && reach.is_some_and(|r| self.extent.width >= r[0] && self.extent.height >= r[1])
        {
            return Ok(true);
        }
        // Without vsync there is no 1:1 present scaling (`build_swapchain`).
        let reach = reach.filter(|_| self.vsync);
        let shape = Shape {
            output: self.output,
            scaling: self.scaling,
            request: reach,
            vsync: self.vsync,
        };
        for slot in 0..SLOTS {
            // SAFETY: forwarded caller contract.
            if !unsafe { self.retire_slot(gpu, slot, self.wait_ns)? } {
                return Ok(false);
            }
        }
        // SAFETY: forwarded caller contract; the queue is idle afterwards, so no
        // present still waits on a per-image semaphore and no blit uses an image.
        unsafe {
            gpu.measure_host_wait(|| gpu.device.queue_wait_idle(gpu.graphics))?;
            let old = self.swapchain;
            let built = build_swapchain(
                gpu,
                &self.loader,
                &self.surface_loader,
                self.surface,
                size,
                old,
                &shape,
            );
            // The old swapchain is retired even when creation fails.
            self.loader.destroy_swapchain(old, None);
            self.swapchain = vk::SwapchainKHR::null();
            let (swapchain, extent, format) = built?;
            self.swapchain = swapchain;
            self.images = self.loader.get_swapchain_images(swapchain)?;
            while self.rendered.len() < self.images.len() {
                self.rendered.push(
                    gpu.device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?,
                );
            }
            self.initialized = vec![false; self.images.len()];
            self.extent = extent;
            self.format = format;
            self.oversized = shape.request.is_some();
            self.built_vsync = shape.vsync;
        }
        Ok(true)
    }
    /// Turns waiting for vertical blank on (FIFO, the default) or off (mailbox where
    /// supported, else immediate). A change rebuilds the swapchain like `resize`; false
    /// while a blit is pending, so call it again on a later frame.
    /// # Safety
    /// As `resize`.
    pub unsafe fn set_vsync(&mut self, gpu: &Gpu, window: &Window, vsync: bool) -> Result<bool> {
        self.vsync = vsync;
        if self.built_vsync == vsync {
            return Ok(true);
        }
        // SAFETY: forwarded caller contract.
        unsafe { self.resize(gpu, window, None) }
    }
    /// Whether presentation waits for vertical blank (`set_vsync`).
    pub fn vsync(&self) -> bool {
        self.vsync
    }

    /// Portable device form of `set_vsync`, used by game settings on either backend.
    /// # Safety
    /// The caller retains this presentation's device and window on their owner thread.
    pub unsafe fn set_vsync_device(
        &mut self,
        gpu: &ara_gpu::GpuDevice,
        window: &Window,
        enabled: bool,
    ) -> Result<bool> {
        // SAFETY: the forwarded contract retains the same native owners.
        unsafe { self.set_vsync(gpu.context(), window, enabled) }
    }
    /// The swapchain's pixel extent.
    pub fn extent(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
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
                    self.wait_ns,
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
    /// Waits, at most `timeout` in all, for every presentation still reading its source:
    /// false while one is pending (a stalled presentation engine left a synchronous
    /// `display` unfinished). The caller must not rewrite or free that source until true.
    /// # Safety
    /// `gpu` created this presentation and is live.
    pub unsafe fn settled(&mut self, gpu: &GpuDevice, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        for slot in 0..SLOTS {
            let left = deadline.saturating_duration_since(Instant::now());
            let nanos = u64::try_from(left.as_nanos()).unwrap_or(u64::MAX);
            // SAFETY: forwarded caller contract.
            if !unsafe { self.retire_slot(gpu.context(), slot, nanos)? } {
                return Ok(false);
            }
        }
        Ok(true)
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
            // A live-resize swapchain (presented 1:1) takes the source unscaled at its
            // top left; otherwise the source is stretched over the whole image.
            let (from, to) = if self.oversized {
                let fit = [
                    size[0].min(self.extent.width),
                    size[1].min(self.extent.height),
                ];
                (fit, fit)
            } else {
                (size, [self.extent.width, self.extent.height])
            };
            let region = vk::ImageBlit::default()
                .src_subresource(layers)
                .dst_subresource(layers)
                .src_offsets([
                    vk::Offset3D::default(),
                    vk::Offset3D {
                        x: from[0] as i32,
                        y: from[1] as i32,
                        z: 1,
                    },
                ])
                .dst_offsets([
                    vk::Offset3D::default(),
                    vk::Offset3D {
                        x: to[0] as i32,
                        y: to[1] as i32,
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
    /// Blit one completed Ara frame to the acquired image and wait for the blit, at most
    /// `PRESENT_WAIT_NS`: a stalled presentation engine leaves it pending, later displays
    /// skip until it completes, and [`Self::settled`] gates the caller's next write.
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
        if let Some(size) = self.pending_size {
            // SAFETY: display and recreation share the caller's native owners.
            if !unsafe { self.resize_to(gpu, size)? } {
                return Ok(());
            }
            self.pending_size = None;
        }
        let slot = self.next;
        // A stalled presentation engine skips frames rather than blocking the caller.
        // SAFETY: this presentation was created on `gpu`.
        if !unsafe { self.retire_slot(gpu, slot, self.wait_ns)? } {
            return Ok(());
        }
        let acquired = self.acquire(gpu, self.slots[slot].acquire);
        if self.native_size.is_some() && acquired.as_ref().is_err_and(is_out_of_date) {
            self.pending_size = self.native_size;
            return Ok(());
        }
        let Some(index) = acquired? else {
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
                self.slots[slot].fence,
            )?;
        }
        let mut result = self.present(gpu, index);
        if self.native_size.is_some() && result.as_ref().is_err_and(is_out_of_date) {
            self.pending_size = self.native_size;
            result = Ok(());
        }
        // The slot's next use waits its fence (`retire_slot`).
        self.slots[slot].command = Some(cmd);
        if self.frames_in_flight {
            self.next = (slot + 1) % SLOTS;
            return result;
        }
        // Synchronous: the blit completes before returning, unless the presentation
        // engine stalls; then the slot stays busy rather than blocking the caller.
        // SAFETY: this presentation was created on `gpu`.
        unsafe { self.retire_slot(gpu, slot, self.wait_ns)? };
        result
    }
}

/// What `build_swapchain` creates.
struct Shape {
    output: OutputSettings,
    /// `Present::scaling`: a requested extent then presents 1:1 at the top left.
    scaling: Option<[vk::Extent2D; 2]>,
    /// An extent other than the window's, honoured only with `scaling`.
    request: Option<[u32; 2]>,
    /// Wait for vertical blank (FIFO). Off: mailbox, else immediate, else FIFO.
    vsync: bool,
}

/// FIFO with `vsync`; otherwise mailbox (no tearing, no wait), else immediate, else FIFO.
/// # Safety
/// `surface` belongs to `gpu`'s instance.
unsafe fn present_mode(
    surface_loader: &ash::khr::surface::Instance,
    gpu: &Gpu,
    surface: vk::SurfaceKHR,
    vsync: bool,
) -> Result<vk::PresentModeKHR> {
    if vsync {
        return Ok(vk::PresentModeKHR::FIFO);
    }
    // SAFETY: forwarded caller contract; the query reads this live surface.
    let modes =
        unsafe { surface_loader.get_physical_device_surface_present_modes(gpu.physical, surface)? };
    Ok([vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::IMMEDIATE]
        .into_iter()
        .find(|mode| modes.contains(mode))
        .unwrap_or(vk::PresentModeKHR::FIFO))
}

/// The image extents `surface` presents 1:1 at the top left (FIFO), if it can.
/// # Safety
/// `surface` belongs to `gpu`'s instance.
unsafe fn one_to_one(gpu: &Gpu, surface: vk::SurfaceKHR) -> Result<Option<[vk::Extent2D; 2]>> {
    if !gpu.info.present_scaling {
        return Ok(None);
    }
    let loader = ash::khr::get_surface_capabilities2::Instance::load(&gpu._entry, &gpu.instance);
    let mut mode = vk::SurfacePresentModeEXT::default().present_mode(vk::PresentModeKHR::FIFO);
    let info = vk::PhysicalDeviceSurfaceInfo2KHR::default()
        .surface(surface)
        .push(&mut mode);
    let mut scaling = vk::SurfacePresentScalingCapabilitiesEXT::default();
    let mut caps = vk::SurfaceCapabilities2KHR::default().push(&mut scaling);
    // SAFETY: forwarded caller contract; the query only fills `caps`.
    unsafe { loader.get_physical_device_surface_capabilities2(gpu.physical, &info, &mut caps)? };
    let top_left = scaling
        .supported_present_scaling
        .contains(vk::PresentScalingFlagsEXT::ONE_TO_ONE)
        && scaling
            .supported_present_gravity_x
            .contains(vk::PresentGravityFlagsEXT::MIN)
        && scaling
            .supported_present_gravity_y
            .contains(vk::PresentGravityFlagsEXT::MIN);
    Ok(top_left.then_some([
        scaling.min_scaled_image_extent,
        scaling.max_scaled_image_extent,
    ]))
}

/// Create a swapchain for `surface`, at its current extent or `shape.request`,
/// retiring `old` (null for the first one) and applying HDR10 metadata when requested.
/// # Safety
/// `surface` belongs to `gpu`'s instance and `window`; `old` is this surface's live
/// swapchain or null.
unsafe fn build_swapchain(
    gpu: &Gpu,
    loader: &ash::khr::swapchain::Device,
    surface_loader: &ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
    size: [u32; 2],
    old: vk::SwapchainKHR,
    shape: &Shape,
) -> Result<(vk::SwapchainKHR, vk::Extent2D, vk::SurfaceFormatKHR)> {
    let output = shape.output;
    ara_gpu::cpu_scope!("Platform.create_swapchain");
    // SAFETY: forwarded caller contract; every query reads this live surface.
    unsafe {
        let caps =
            surface_loader.get_physical_device_surface_capabilities(gpu.physical, surface)?;
        ensure!(
            caps.supported_usage_flags
                .contains(vk::ImageUsageFlags::TRANSFER_DST),
            "Swapchain must support transfer destination"
        );
        let formats = surface_loader.get_physical_device_surface_formats(gpu.physical, surface)?;
        let format = select_format(&formats, output.mode)
            .with_context(|| format!("Requested {:?} output unavailable; enable Windows HDR and use an HDR display. Advertised formats: {formats:?}", output.mode))?;
        let extent = if caps.current_extent.width == u32::MAX {
            vk::Extent2D {
                width: size[0].clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: size[1].clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        } else {
            caps.current_extent
        };
        // Present scaling was queried for FIFO; without vsync the swapchain matches
        // the window instead of presenting a larger one 1:1.
        let scaled = shape.scaling.filter(|_| shape.vsync);
        let extent = match (scaled, shape.request) {
            (Some([min, max]), Some(request)) => vk::Extent2D {
                width: request[0]
                    .min(gpu.info.max_image_dimension)
                    .clamp(min.width, max.width),
                height: request[1]
                    .min(gpu.info.max_image_dimension)
                    .clamp(min.height, max.height),
            },
            _ => extent,
        };
        let mut scaling = vk::SwapchainPresentScalingCreateInfoEXT::default()
            .scaling_behavior(vk::PresentScalingFlagsEXT::ONE_TO_ONE)
            .present_gravity_x(vk::PresentGravityFlagsEXT::MIN)
            .present_gravity_y(vk::PresentGravityFlagsEXT::MIN);
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
        let mut info = vk::SwapchainCreateInfoKHR::default()
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
            .present_mode(present_mode(surface_loader, gpu, surface, shape.vsync)?)
            .clipped(true)
            .old_swapchain(old);
        if scaled.is_some() && shape.request.is_some() {
            info = info.push(&mut scaling);
        }
        let swapchain = loader.create_swapchain(&info, None)?;
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
        Ok((swapchain, extent, format))
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

/// Resize races are retried by native layers on their next display pass.
fn is_out_of_date(error: &anyhow::Error) -> bool {
    error.downcast_ref::<vk::Result>() == Some(&vk::Result::ERROR_OUT_OF_DATE_KHR)
}
