use crate::{
    gpu::{Gpu, Image, image_barrier},
    render::{HEIGHT, WIDTH},
};
use anyhow::{Context, Result, ensure};
use ash::vk;
use ash::vk::Handle;
use sdl3_sys::{error::SDL_GetError, events::*, init::*, video::*, vulkan::*};
use std::ffi::{CStr, c_char};

/// SDL video lifetime and the fixed-size native Vulkan window.
pub struct Window {
    handle: *mut SDL_Window,
}

impl Window {
    pub fn new() -> Result<Self> {
        // SAFETY: Main thread owns the SDL video lifetime; window is destroyed on drop.
        unsafe {
            ensure!(SDL_Init(SDL_INIT_VIDEO), "SDL init: {}", error());
            let handle = SDL_CreateWindow(
                c"Ara S1: Vulkan bindless + inline RT + async compute".as_ptr(),
                WIDTH as i32,
                HEIGHT as i32,
                SDL_WINDOW_VULKAN,
            );
            ensure!(!handle.is_null(), "SDL window: {}", error());
            Ok(Self { handle })
        }
    }
    pub fn extensions(&self) -> Result<Vec<*const c_char>> {
        let mut count = 0;
        // SAFETY: SDL initialized Vulkan for this window; returned static strings
        // outlive instance creation, and count is supplied by SDL.
        unsafe {
            let names = SDL_Vulkan_GetInstanceExtensions(&mut count);
            ensure!(!names.is_null(), "SDL Vulkan extensions: {}", error());
            Ok(std::slice::from_raw_parts(names, count as usize).to_vec())
        }
    }
    pub fn poll(&self) -> bool {
        // SAFETY: SDL fills the event union; only common type field is read.
        unsafe {
            let mut event = std::mem::MaybeUninit::<SDL_Event>::uninit();
            while SDL_PollEvent(event.as_mut_ptr()) {
                if (*event.as_ptr()).r#type == SDL_EVENT_QUIT.0 {
                    return false;
                }
            }
        }
        true
    }
}

fn error() -> String {
    // SAFETY: SDL_GetError always returns a valid nul-terminated thread-local string.
    unsafe {
        CStr::from_ptr(SDL_GetError())
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: The window is owned here; caller destroys the surface first.
        unsafe {
            SDL_DestroyWindow(self.handle);
            SDL_Quit();
        }
    }
}

/// Swapchain, surface and per-image presentation semaphore ownership.
pub struct Present {
    pub surface: vk::SurfaceKHR,
    pub swapchain: vk::SwapchainKHR,
    pub loader: ash::khr::swapchain::Device,
    pub acquire: vk::Semaphore,
    rendered: Vec<vk::Semaphore>,
    images: Vec<vk::Image>,
    initialized: Vec<bool>,
    extent: vk::Extent2D,
    surface_loader: ash::khr::surface::Instance,
}

impl Present {
    pub fn new(gpu: &Gpu, window: &Window) -> Result<Self> {
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
            let format = formats
                .iter()
                .find(|f| {
                    f.format == vk::Format::B8G8R8A8_UNORM || f.format == vk::Format::R8G8B8A8_UNORM
                })
                .context("No UNORM swapchain format")?;
            let extent = if caps.current_extent.width == u32::MAX {
                vk::Extent2D {
                    width: WIDTH,
                    height: HEIGHT,
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
            let images = loader.get_swapchain_images(swapchain)?;
            let acquire = gpu
                .device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
            let rendered = (0..images.len())
                .map(|_| {
                    gpu.device
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let initialized = vec![false; images.len()];
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
            })
        }
    }
    pub fn acquire(&self) -> Result<u32> {
        // SAFETY: The prior frame timeline has completed, so acquire is reusable.
        unsafe {
            Ok(self
                .loader
                .acquire_next_image(self.swapchain, u64::MAX, self.acquire, vk::Fence::null())?
                .0)
        }
    }
    pub fn rendered(&self, index: u32) -> vk::Semaphore {
        self.rendered[index as usize]
    }
    pub fn record(&mut self, gpu: &Gpu, cmd: vk::CommandBuffer, source: Image, index: u32) {
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
                        x: WIDTH as i32,
                        y: HEIGHT as i32,
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
                source.handle,
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
    pub fn present(&self, gpu: &Gpu, index: u32) -> Result<()> {
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
    pub fn destroy(&self, gpu: &Gpu) {
        // SAFETY: Called after device idle (including present), before window/instance drop.
        unsafe {
            gpu.device.destroy_semaphore(self.acquire, None);
            for semaphore in &self.rendered {
                gpu.device.destroy_semaphore(*semaphore, None);
            }
            self.loader.destroy_swapchain(self.swapchain, None);
            self.surface_loader.destroy_surface(self.surface, None);
        }
    }
}
