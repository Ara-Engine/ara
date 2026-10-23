use super::*;
impl Landscape {
    /// Diagnostic-only readback after a completed frame; production draws use GPU counts.
    pub fn visibility_stats(&self, gpu: &mut VulkanDevice) -> Result<VisibilityStats> {
        let mut counts = Vec::new();
        for (source, count) in [(self.args, 6), (self.foliage_args, 9)] {
            let read = gpu.create_buffer(count * 16, vk::BufferUsageFlags::TRANSFER_DST, true)?;
            let mut r = gpu.begin(Queue::Graphics);
            r.barriers([ara_gpu::Barrier {
                resource: Resource::Buffer(source),
                before: Access::Indirect,
                after: Access::CopySource,
                handoff: None,
            }]);
            r.push(Command::CopyBuffer {
                source,
                destination: read,
                size: count * 16,
            });
            r.barriers([
                ara_gpu::Barrier {
                    resource: Resource::Buffer(source),
                    before: Access::CopySource,
                    after: Access::Indirect,
                    handoff: None,
                },
                ara_gpu::Barrier {
                    resource: Resource::Buffer(read),
                    before: Access::CopyDestination,
                    after: Access::HostRead,
                    handoff: None,
                },
            ]);
            let p = gpu.submit(r.finish(), &[])?;
            gpu.resolve(p)?;
            let bytes = gpu.read(read)?;
            for record in bytes.as_chunks::<16>().0 {
                counts.push(u32::from_le_bytes(record[4..8].try_into()?));
            }
            gpu.retire(Resource::Buffer(read))?;
            gpu.collect()?;
        }
        Ok(VisibilityStats {
            terrain_patches: counts[..6].try_into()?,
            foliage_instances: counts[6..].try_into()?,
        })
    }
    /// Diagnostic-only synchronous capture. Production callers use `record`.
    pub fn capture(&mut self, gpu: &mut VulkanDevice, path: &Path) -> Result<()> {
        let read = gpu.create_buffer(
            u64::from(self.size[0]) * u64::from(self.size[1]) * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut r = gpu.begin(Queue::Graphics);
        r.barriers([ara_gpu::Barrier {
            resource: Resource::Image(self.color),
            before: Access::Color,
            after: Access::CopySource,
            handoff: None,
        }]);
        r.push(Command::ReadImage {
            source: self.color,
            destination: read,
        });
        r.barriers([
            ara_gpu::Barrier {
                resource: Resource::Buffer(read),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
            ara_gpu::Barrier {
                resource: Resource::Image(self.color),
                before: Access::CopySource,
                after: Access::Color,
                handoff: None,
            },
        ]);
        let p = gpu.submit(r.finish(), &[])?;
        gpu.resolve(p)?;
        image::save_buffer(
            path,
            &gpu.read(read)?,
            self.size[0],
            self.size[1],
            image::ColorType::Rgba8,
        )?;
        gpu.retire(Resource::Buffer(read))?;
        gpu.collect()?;
        Ok(())
    }
    /// Retire all scene resources after the host has finished its last submission.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        for b in [
            self.frame,
            self.samples,
            self.instances,
            self.levels,
            self.lists,
            self.args,
            self.visible,
            self.foliage_args,
            self.parameters,
            self.upload,
        ] {
            gpu.retire(Resource::Buffer(b))?;
        }
        if let Some(geometry) = self.geometry {
            gpu.retire(Resource::Buffer(geometry))?;
        }
        for image in
            self.textures
                .into_iter()
                .chain([self.color, self.depth, self.shadow, self.hzb])
        {
            gpu.retire(Resource::Image(image))?;
        }
        for p in self.pipelines.into_values() {
            gpu.retire_pipeline(p)?;
        }
        gpu.collect()?;
        Ok(())
    }
}
