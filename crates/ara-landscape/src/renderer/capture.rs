use super::*;
impl Landscape {
    /// Diagnostic-only readback after a completed frame; production draws use GPU counts.
    pub fn visibility_stats(&self, gpu: &mut GpuDevice) -> Result<VisibilityStats> {
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
    pub fn capture(&mut self, gpu: &mut GpuDevice, path: &Path) -> Result<()> {
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
    /// Synchronous readback of the last resolved frame for host composition: display
    /// colour (RGBA8, row-major) and reversed-Z depth (0 = sky). Waits for the GPU.
    pub fn read_color_depth(&mut self, gpu: &mut GpuDevice) -> Result<(Vec<u8>, Vec<f32>)> {
        let pixels = u64::from(self.size[0]) * u64::from(self.size[1]);
        let color_read = gpu.create_buffer(pixels * 4, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        let depth_read = gpu.create_buffer(pixels * 4, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        // Graph import order: nine buffers, then colour (9) and depth (10).
        let state =
            |i: usize, fallback: Access| self.previous.get(i).map_or(fallback, |s| s.access);
        let (color_state, depth_state) = (state(9, Access::Color), state(10, Access::Depth));
        let mut r = gpu.begin(Queue::Graphics);
        r.barriers([
            ara_gpu::Barrier {
                resource: Resource::Image(self.color),
                before: color_state,
                after: Access::CopySource,
                handoff: None,
            },
            ara_gpu::Barrier {
                resource: Resource::Image(self.depth),
                before: depth_state,
                after: Access::CopySource,
                handoff: None,
            },
        ]);
        r.push(Command::ReadImage {
            source: self.color,
            destination: color_read,
        });
        r.push(Command::ReadImage {
            source: self.depth,
            destination: depth_read,
        });
        r.barriers([
            ara_gpu::Barrier {
                resource: Resource::Buffer(color_read),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
            ara_gpu::Barrier {
                resource: Resource::Buffer(depth_read),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
            ara_gpu::Barrier {
                resource: Resource::Image(self.color),
                before: Access::CopySource,
                after: color_state,
                handoff: None,
            },
            ara_gpu::Barrier {
                resource: Resource::Image(self.depth),
                before: Access::CopySource,
                after: depth_state,
                handoff: None,
            },
        ]);
        let p = gpu.submit(r.finish(), &[])?;
        gpu.resolve(p)?;
        let color = gpu.read(color_read)?;
        let depth = gpu
            .read(depth_read)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        gpu.retire(Resource::Buffer(color_read))?;
        gpu.retire(Resource::Buffer(depth_read))?;
        gpu.collect()?;
        Ok((color, depth))
    }
    /// Retire all scene resources after the host has finished its last submission.
    pub fn retire(self, gpu: &mut GpuDevice) -> Result<()> {
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
