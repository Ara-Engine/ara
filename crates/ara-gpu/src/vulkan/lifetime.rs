use super::*;

impl Device for VulkanDevice {
    fn retire_pipeline(&mut self, pipeline: Pipeline) -> Result<()> {
        VulkanDevice::retire_pipeline(self, pipeline)
    }
    fn create_acceleration_structure(
        &mut self,
        desc: &crate::acceleration::AccelerationDesc,
    ) -> Result<AccelerationStructure> {
        VulkanDevice::create_acceleration_structure(self, desc)
    }
    fn queues(&self) -> [descriptors::QueueInfo; 2] {
        [Queue::Graphics, Queue::Compute].map(|id| descriptors::QueueInfo {
            id,
            dedicated: id == Queue::Compute,
            timestamps: descriptors::TimestampCaps {
                valid_bits: self.context.info.timestamp_valid_bits[id.index()],
                period_ns: self.context.info.timestamp_period_ns,
            },
        })
    }
    fn create_buffer(&mut self, desc: &descriptors::BufferDesc) -> Result<Buffer> {
        self.portable_buffer(desc)
    }
    fn create_image(&mut self, desc: &descriptors::ImageDesc) -> Result<Image> {
        self.portable_image(desc)
    }
    fn create_pipeline(
        &mut self,
        cooked: &cooked::CookedPipeline,
        artifacts: &std::path::Path,
    ) -> Result<Pipeline> {
        VulkanDevice::create_pipeline(self, cooked, artifacts)
    }
    fn heap(&mut self) -> &mut impl BindlessHeap {
        self
    }
    fn begin(&self, queue: Queue) -> Recorder {
        VulkanDevice::begin(self, queue)
    }
    fn submit(&mut self, commands: CommandList, waits: &[TimelinePoint]) -> Result<TimelinePoint> {
        VulkanDevice::submit(self, commands, waits)
    }
    fn completed(&self) -> Result<[u64; 2]> {
        // SAFETY: Both timeline semaphores belong to this live device.
        unsafe {
            Ok([
                self.context
                    .device
                    .get_semaphore_counter_value(self.timelines[0])
                    .map_err(anyhow::Error::from)?,
                self.context
                    .device
                    .get_semaphore_counter_value(self.timelines[1])
                    .map_err(anyhow::Error::from)?,
            ])
        }
    }
    fn wait(&self, point: TimelinePoint) -> Result<()> {
        if point.value > self.submitted[point.queue.index()] {
            return Err(GpuError::Contract(
                "Cannot wait on an unsubmitted timeline point".into(),
            ));
        }
        // SAFETY: Point was signalled on this device; no object is destroyed during wait.
        unsafe {
            self.context.device.wait_semaphores(
                &vk::SemaphoreWaitInfo::default()
                    .semaphores(&[self.timelines[point.queue.index()]])
                    .values(&[point.value]),
                u64::MAX,
            )
        }
        .map_err(anyhow::Error::from)?;
        Ok(())
    }
    fn retire(&mut self, resource: Resource) -> Result<()> {
        let retired = match resource {
            Resource::Buffer(b) => Retired::Buffer(self.buffers.remove(b)?),
            Resource::Image(i) => Retired::Image(self.images.remove(i)?),
            Resource::AccelerationStructure(h) => {
                if self
                    .accelerations
                    .values_mut()
                    .any(|v| v.raw.dependencies.contains(&h))
                {
                    return Err(GpuError::Contract(
                        "BLAS is referenced by a live TLAS".into(),
                    ));
                }
                let native = self.accelerations.remove(h)?;
                for &buffer in &native.raw.auxiliary {
                    self.retired
                        .push(Retired::Buffer(self.buffers.remove(buffer)?));
                }
                Retired::Acceleration(native)
            }
        };
        self.retired.push(retired);
        Ok(())
    }
    fn collect(&mut self) -> Result<()> {
        let done = self.completed()?;
        let mut index = 0;
        while index < self.retired.len() {
            if (0..2).all(|q| done[q] >= self.retired[index].last()[q]) {
                match self.retired.swap_remove(index) {
                    Retired::Buffer(b) => {
                        self.heap.retire_buffer(b.heap);
                        self.context.destroy_buffer(b.raw);
                    }
                    Retired::Pipeline(pipeline, _) => {
                        // SAFETY: All submissions that could reference this pipeline have completed.
                        unsafe {
                            self.context.device.destroy_pipeline(pipeline.handle, None);
                        }
                    }
                    Retired::Acceleration(a) => {
                        self.heap.retire_acceleration(a.heap);
                        self.context.destroy_structure(a.raw.handle, a.raw.backing);
                    }
                    Retired::Image(i) => {
                        self.heap.retire_image(i.raw.view, i.heap);
                        self.context.destroy_image(i.raw);
                    }
                }
            } else {
                index += 1;
            }
        }
        Ok(())
    }
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        // SAFETY: Last owner. Idle covers descriptors, pipelines, queries and command buffers.
        unsafe {
            let _ = self.context.device.device_wait_idle();
            if let Ok(bytes) = self.context.device.get_pipeline_cache_data(self.cache) {
                let _ = cache::persist(&self.cache_path, &bytes);
            }
            for pipeline in self.pipelines.values_mut() {
                self.context.device.destroy_pipeline(pipeline.handle, None);
            }
            self.context.device.destroy_pipeline_cache(self.cache, None);
            for retired in &self.retired {
                if let Retired::Pipeline(pipeline, _) = retired {
                    self.context.device.destroy_pipeline(pipeline.handle, None);
                }
            }
            for pending in &self.pending {
                self.context.device.destroy_query_pool(pending.query, None);
            }
            self.heap.destroy(&self.context);
            for semaphore in self.timelines {
                self.context.device.destroy_semaphore(semaphore, None);
            }
        }
    }
}
