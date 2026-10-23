use super::*;
impl Residency {
    /// Reads completed GPU feedback counters and reports owned texture memory.
    pub fn stats(&self, gpu: &VulkanDevice) -> Result<Stats> {
        let bytes = gpu.read(self.buffers[6])?;
        let word = |i: usize| {
            u32::from_le_bytes(
                bytes[i * 4..i * 4 + 4]
                    .try_into()
                    .expect("validated feedback allocation"),
            )
        };
        Ok(Stats {
            virtual_pages: self.catalog.page_count() as u32,
            resident_pages: self.pool.resident(),
            pool_bytes: self.pool.bytes(),
            auxiliary_bytes: (self.catalog.page_count() * 12
                + 128 * 8
                + self.catalog.sources.len() * 32
                + self.upload_pages * PACKET_WORDS * 4) as u64
                + u64::from(self.size[0]) * u64::from(self.size[1]) * 16,
            source_bytes: self
                .catalog
                .sources
                .iter()
                .flat_map(|s| &s.levels)
                .map(|m| m.len() as u64)
                .sum(),
            uploaded_pages: self.pending,
            samples: word(0),
            samples_over_one_mip: word(1),
            max_mip_lag: word(2) as f32 / 256.,
        })
    }
    /// Readback buffer for caller-owned diagnostic copy of UV/desired/actual mip.
    /// The returned GPU buffer rests in Storage; normal rendering never reads it.
    pub fn diagnostics(&self) -> Buffer {
        self.buffers[5]
    }
    /// Diagnostic-only synchronous UV/desired/actual mip readback. Call after
    /// end_frame completes. Uncovered pixels are undefined; use surface coverage.
    pub fn capture_samples(&self, gpu: &mut VulkanDevice) -> Result<Vec<[f32; 4]>> {
        let size = u64::from(self.size[0]) * u64::from(self.size[1]) * 16;
        let read = gpu.create_buffer(size, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        let mut r = gpu.begin(Queue::Graphics);
        r.barriers([ara_gpu::Barrier {
            resource: Resource::Buffer(self.buffers[5]),
            before: Access::Storage,
            after: Access::CopySource,
            handoff: None,
        }]);
        r.push(Command::CopyBuffer {
            source: self.buffers[5],
            destination: read,
            size,
        });
        r.barriers([
            ara_gpu::Barrier {
                resource: Resource::Buffer(self.buffers[5]),
                before: Access::CopySource,
                after: Access::Storage,
                handoff: None,
            },
            ara_gpu::Barrier {
                resource: Resource::Buffer(read),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
        ]);
        let point = gpu.submit(r.finish(), &[])?;
        gpu.resolve(point)?;
        let pixels = gpu
            .read(read)?
            .as_chunks::<16>()
            .0
            .iter()
            .map(|bytes| bytemuck::pod_read_unaligned(bytes))
            .collect();
        gpu.retire(Resource::Buffer(read))?;
        gpu.collect()?;
        Ok(pixels)
    }
    /// Retires physical pages and all feedback/staging/pipeline allocations.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        for buffer in self.buffers {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        for pipeline in self.pipelines {
            gpu.retire_pipeline(pipeline)?;
        }
        gpu.collect()?;
        Ok(())
    }
}
