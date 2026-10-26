//! One native device and swapchain, with exclusive command pools for loading.
use super::*;

impl Gpu {
    pub(crate) fn presentation_only(&self) -> bool {
        self.presentation_only
    }
    pub(crate) fn split_presentation(&mut self) -> Result<Self> {
        anyhow::ensure!(!self.presentation_only, "Loading context cannot be split");
        // SAFETY: Allocation does not access a queue. Both new pools belong only
        // to the returned owner, and native lifetime is retained independently.
        let (graphics_pool, compute_pool) = unsafe {
            let create = |family| {
                self.device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
            };
            let graphics = create(self.info.graphics_family)?;
            match create(self.info.compute_family) {
                Ok(compute) => (graphics, compute),
                Err(error) => {
                    self.device.destroy_command_pool(graphics, None);
                    return Err(error.into());
                }
            }
        };
        let loading = Self {
            host_waits: Default::default(),
            shared: Arc::clone(&self.shared),
            presentation_only: true,
            graphics: self.graphics,
            scene_graphics: self.graphics,
            compute: self.compute,
            graphics_pool,
            compute_pool,
            buffers: Vec::new(),
            buffer_memory: Default::default(),
            images: Vec::new(),
            structures: Vec::new(),
        };
        // Game work uses queue 1 where available; queue 0 retains presentation.
        // On a one-queue adapter the caller serializes the two owners instead.
        self.graphics = self.scene_graphics;
        Ok(loading)
    }

    /// # Safety
    /// No other thread may submit to these owned queues during the wait. A
    /// presentation-only owner never touches the game's dedicated compute queue.
    pub(crate) unsafe fn wait_owned_queues(&self) -> Result<()> {
        let queues = [self.graphics, self.scene_graphics, self.compute];
        let count = if self.presentation_only { 1 } else { 3 };
        for (index, &queue) in queues[..count].iter().enumerate() {
            if !queues[..index].contains(&queue) {
                // SAFETY: Caller holds this context's exclusive queue ownership.
                unsafe { self.device.queue_wait_idle(queue)? };
            }
        }
        Ok(())
    }
}
