use crate::output::select_format;
use crate::window::{Window, error};
use anyhow::{Context, Result, ensure};
use ara_color::{OutputMode, OutputSettings};
use ara_gpu::vk::Handle;
use ara_gpu::{
    vk,
    vulkan::{Context as Gpu, image_barrier},
};
use sdl3_sys::vulkan::SDL_Vulkan_CreateSurface;
/// Swapchain, surface and per-image presentation semaphore ownership.
pub struct Present {
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    loader: ash::khr::swapchain::Device,
    acquire: vk::Semaphore,
    rendered: Vec<vk::Semaphore>,
    images: Vec<vk::Image>,
    initialized: Vec<bool>,
    extent: vk::Extent2D,
    surface_loader: ash::khr::surface::Instance,
    output: OutputSettings,
    format: vk::SurfaceFormatKHR,
}

impl Present {
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
            let acquire = gpu
                .device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
            guard.semaphores.push(acquire);
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
                acquire,
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
    /// Acquire the next image; OUT_OF_DATE is returned for caller recreation.
    fn acquire(&self) -> Result<u32> {
        // SAFETY: The prior frame timeline has completed, so acquire is reusable.
        unsafe {
            Ok(self
                .loader
                .acquire_next_image(self.swapchain, u64::MAX, self.acquire, vk::Fence::null())?
                .0)
        }
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
        // SAFETY: Submit signals the semaphore for this acquired image. Reusing
        // per-image semaphores only after reacquisition also proves present consumed it.
        unsafe {
            self.loader.queue_present(
                gpu.graphics,
                &vk::PresentInfoKHR::default()
                    .wait_semaphores(&[self.rendered(index)])
                    .swapchains(&[self.swapchain])
                    .image_indices(&[index]),
            )?;
        }
        Ok(())
    }
    /// Destroy after device idle, before the SDL window or Vulkan instance.
    /// # Safety
    /// Pass the creating device after all its queues and presentation work are idle.
    pub unsafe fn destroy(&mut self, gpu: &Gpu) {
        // SAFETY: Called after device idle (including present), before window/instance drop.
        unsafe {
            if self.acquire != vk::Semaphore::null() {
                gpu.device.destroy_semaphore(self.acquire, None);
                self.acquire = vk::Semaphore::null();
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
        ensure!(
            encoding == self.output.mode,
            "Presentation/output encoding mismatch"
        );
        ensure!(
            self.swapchain != vk::SwapchainKHR::null(),
            "Presentation has been destroyed"
        );
        let index = self.acquire()?;
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
                .semaphore(self.acquire)
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
                vk::Fence::null(),
            )?;
        }
        let result = self.present(gpu, index);
        // SAFETY: Waiting includes the blit/present queue operations before command reuse/free.
        unsafe {
            gpu.device.queue_wait_idle(gpu.graphics)?;
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
            if self.swapchain != vk::SwapchainKHR::null() {
                ash::khr::swapchain::Device::load(&self.gpu.instance, &self.gpu.device)
                    .destroy_swapchain(self.swapchain, None);
            }
            ash::khr::surface::Instance::load(&self.gpu._entry, &self.gpu.instance)
                .destroy_surface(self.surface, None);
        }
    }
}
