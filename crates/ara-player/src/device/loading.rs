//! A bounded transfer-only loading view. It never opens cooked shaders, creates
//! a pipeline, or waits for the game presenter's device to finish warming.
use super::*;
use ara_gpu::vulkan::VulkanDevice;
use ara_gpu::{Access, Barrier, Buffer, Device, Image, Queue, Resource, commands::Command, vk};

const SIZE: [u32; 2] = [320, 180];
#[cfg(test)]
mod tests;

/// One 225 KiB image and staging buffer with independent loading queue/pools.
/// The logical device and swapchain survive the transition to game presentation.
pub(crate) struct Loading {
    gpu: VulkanDevice,
    image: Image,
    staging: Buffer,
    pixels: Vec<u8>,
    initialized: bool,
    frames: usize,
    present: Option<Present>,
}

impl Loading {
    #[cfg(test)]
    pub(crate) fn native_device(&self) -> vk::Device {
        self.gpu.context().device.handle()
    }

    #[cfg(test)]
    pub(crate) fn swapchain(&self) -> Option<vk::SwapchainKHR> {
        self.present.as_ref().map(Present::native_swapchain)
    }
    #[cfg(test)]
    pub(crate) fn new(extensions: &[std::ffi::CString], cache: &Path) -> Result<Self> {
        ara_gpu::cpu_scope!("Player.loading_device");
        Self::with_device(VulkanDevice::new_raster(extensions, cache.join("loading"))?)
    }

    pub(crate) fn for_game(
        extensions: &[std::ffi::CString],
        cache: &Path,
        ray_tracing: bool,
    ) -> Result<(Self, VulkanDevice, bool)> {
        let mut game = if ray_tracing {
            VulkanDevice::new(extensions, cache)?
        } else {
            VulkanDevice::new_raster(extensions, cache)?
        };
        // SAFETY: No work has been submitted. The caller pauses loading queue
        // access during worker preparation when the adapter has only one queue.
        let loading = unsafe { game.split_loading(cache.join("loading"))? };
        let parallel = loading.context().graphics != game.context().graphics;
        Ok((Self::with_device(loading)?, game, parallel))
    }

    fn with_device(mut gpu: VulkanDevice) -> Result<Self> {
        let bytes = u64::from(SIZE[0]) * u64::from(SIZE[1]) * 4;
        let image = gpu.create_image(
            SIZE[0],
            SIZE[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::SAMPLED,
        )?;
        let staging = gpu.create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
        Ok(Self {
            gpu,
            image,
            staging,
            pixels: vec![0; bytes as usize],
            initialized: false,
            frames: 0,
            present: None,
        })
    }

    fn draw(&mut self, progress: f32) -> Result<()> {
        ara_gpu::cpu_scope!("Player.loading_frame");
        for pixel in self.pixels.as_chunks_mut::<4>().0 {
            *pixel = [7, 11, 18, 255];
        }
        let end = 104 + (112. * progress.clamp(0., 1.)) as usize;
        let pulse = end + (self.frames / 3) % (216 - end).max(1);
        for y in 89..92 {
            for x in 104..216 {
                let pixel = (y * SIZE[0] as usize + x) * 4;
                self.pixels[pixel..pixel + 4].copy_from_slice(if x < end {
                    &[43, 123, 230, 255]
                } else if (pulse..pulse + 4).contains(&x) {
                    &[74, 151, 240, 255]
                } else {
                    &[22, 32, 47, 255]
                });
            }
        }
        self.gpu.upload(self.staging, &self.pixels)?;
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.barriers([
            barrier(
                Resource::Buffer(self.staging),
                Access::HostWrite,
                Access::CopySource,
            ),
            barrier(
                Resource::Image(self.image),
                if self.initialized {
                    Access::CopySource
                } else {
                    Access::Undefined
                },
                Access::CopyDestination,
            ),
        ]);
        commands.push(Command::UploadImage {
            source: self.staging,
            destination: self.image,
        });
        commands.barriers([
            barrier(
                Resource::Image(self.image),
                Access::CopyDestination,
                Access::CopySource,
            ),
            barrier(
                Resource::Buffer(self.staging),
                Access::CopySource,
                Access::HostWrite,
            ),
        ]);
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.gpu.resolve(point)?;
        self.initialized = true;
        self.frames = self.frames.wrapping_add(1);
        self.gpu.assert_clean()?;
        Ok(())
    }

    /// Explicit evidence only: read the uploaded loading image, never game targets.
    pub(crate) fn capture(&mut self, path: &Path, display: [u32; 2]) -> Result<()> {
        let pixels = self.read_pixels()?;
        let image =
            image::RgbaImage::from_raw(SIZE[0], SIZE[1], pixels).context("loading image extent")?;
        image::imageops::resize(
            &image,
            display[0],
            display[1],
            image::imageops::FilterType::Nearest,
        )
        .save(path)?;
        Ok(())
    }

    fn read_pixels(&mut self) -> Result<Vec<u8>> {
        let buffer = self.gpu.create_buffer(
            self.pixels.len() as u64,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let result = (|| {
            let mut commands = self.gpu.begin(Queue::Graphics);
            commands.barriers([barrier(
                Resource::Buffer(buffer),
                Access::Undefined,
                Access::CopyDestination,
            )]);
            commands.push(Command::ReadImage {
                source: self.image,
                destination: buffer,
            });
            commands.barriers([barrier(
                Resource::Buffer(buffer),
                Access::CopyDestination,
                Access::HostRead,
            )]);
            let point = self.gpu.submit(commands.finish(), &[])?;
            self.gpu.resolve(point)?;
            let pixels = self.gpu.read(buffer)?;
            self.gpu.assert_clean()?;
            Ok(pixels)
        })();
        self.gpu.retire(Resource::Buffer(buffer))?;
        self.gpu.collect()?;
        result
    }
}

impl Windowed {
    pub(crate) fn drawable_size(&self) -> Result<[u32; 2]> {
        self.window.size()
    }
    #[cfg(test)]
    pub(crate) fn swapchain(&self) -> Option<vk::SwapchainKHR> {
        self.present.as_ref().map(Present::native_swapchain)
    }
    /// Transfer the existing swapchain after worker completion. Both contexts
    /// refer to the same native device; no surface or swapchain is recreated.
    pub(crate) fn finish_loading(&mut self, mut loading: Loading) {
        self.present = loading.present.take();
    }
    /// Create only SDL ownership; shader compilation belongs to a separate worker.
    pub(crate) fn open_unprepared(
        name: &str,
        size: [u32; 2],
        hidden: bool,
    ) -> Result<(Self, Vec<std::ffi::CString>, [u32; 2])> {
        Self::open_unprepared_with_mode(name, size, hidden, ara_platform::GameWindowMode::Windowed)
    }

    /// Standalone games select desktop fullscreen; pooled editor windows keep
    /// their independent requested extents. Hidden probes never enter fullscreen.
    pub(crate) fn open_unprepared_with_mode(
        name: &str,
        size: [u32; 2],
        hidden: bool,
        mode: ara_platform::GameWindowMode,
    ) -> Result<(Self, Vec<std::ffi::CString>, [u32; 2])> {
        let title = format!("Ara | {name}");
        let window = Window::game(&title, size, mode, hidden)?;
        let size = window.size()?;
        let extensions = window.extensions()?;
        Ok((
            Self {
                window,
                present: None,
                present2d: None,
                input: Mapper::default(),
                frames: 0,
                frame_times: Vec::new(),
                window_start: Instant::now(),
                present_enabled: true,
            },
            extensions,
            size,
        ))
    }

    pub(crate) fn present_loading(&mut self, loading: &mut Loading, progress: f32) -> Result<bool> {
        if self.window.size()?.contains(&0) {
            return Ok(false);
        }
        loading.draw(progress)?;
        if !self.present_enabled {
            return Ok(true);
        }
        if loading.present.is_none() {
            // SAFETY: This window and shared device outlive the swapchain, which
            // is transferred to the game owner after completed worker preparation.
            loading.present = Some(unsafe { Present::new(loading.gpu.context(), &self.window)? });
        }
        let image = loading.gpu.native_image(loading.image)?;
        // SAFETY: draw completed this CopySource image. The worker uses a distinct
        // queue, or the caller serializes both owners on single-queue adapters.
        let result = unsafe {
            loading
                .present
                .as_mut()
                .context("loading swapchain")?
                .display(loading.gpu.context(), image, SIZE)
        };
        if result
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<vk::Result>())
            == Some(&vk::Result::ERROR_OUT_OF_DATE_KHR)
        {
            loading.clear_present();
            Ok(false)
        } else {
            result.map(|()| true)
        }
    }
}

impl Loading {
    fn clear_present(&mut self) {
        if let Some(mut present) = self.present.take() {
            // SAFETY: Every display completed before returning. The worker uses
            // neither these semaphores nor this surface; no global queue wait
            // may block close/resize behind the game's uploads.
            unsafe {
                present.destroy(self.gpu.context());
            }
        }
    }
}
impl Drop for Loading {
    fn drop(&mut self) {
        self.clear_present();
    }
}

fn barrier(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
