use super::*;

pub(in crate::vulkan) struct Raw {
    device: ash::Device,
    buffers: Vec<BufferAllocation>,
    images: Vec<ImageAllocation>,
}

impl Raw {
    pub fn create(
        allocator: &allocation::Allocator,
        request: ResourcePreparation,
    ) -> Result<Box<Self>> {
        crate::cpu_scope!("GPU.prepare_resources");
        let mut result = Box::new(Self {
            device: allocator.device.clone(),
            buffers: Vec::with_capacity(request.buffers.len()),
            images: Vec::with_capacity(request.images.len()),
        });
        for buffer in request.buffers {
            if buffer.size == 0
                || buffer.usage.is_empty()
                || (!buffer.host && !buffer.data.is_empty())
            {
                return Err(GpuError::Contract("Invalid prepared buffer".into()));
            }
            let data = buffer.data.load()?;
            if data.len() as u64 > buffer.size {
                return Err(GpuError::Contract(
                    "Prepared buffer data exceeds capacity".into(),
                ));
            }
            let usage = buffer.usage
                | if buffer.host {
                    vk::BufferUsageFlags::TRANSFER_DST
                } else {
                    vk::BufferUsageFlags::empty()
                };
            let raw = allocator.buffer(buffer.size, usage, buffer.host)?;
            result.buffers.push(raw);
            if !data.is_empty() {
                allocation::upload(&allocator.device, raw, 0, &data)?;
            }
        }
        for image in request.images {
            result
                .images
                .push(allocator.image(allocation::ImageDesc::new(
                    image.size,
                    image.format,
                    image.mip_levels,
                    image.usage,
                ))?);
        }
        Ok(result)
    }

    pub fn dispose(mut self, disposal: &mut disposal::Disposal) {
        for image in self.images.drain(..) {
            disposal.push(disposal::Object::Image(image));
        }
        for buffer in self.buffers.drain(..) {
            disposal.push(disposal::Object::Buffer(buffer));
        }
    }

    pub fn adopt(mut self, gpu: &mut VulkanDevice) -> Result<PreparedResources> {
        let mut result = PreparedResources {
            buffers: Vec::with_capacity(self.buffers.len()),
            images: Vec::with_capacity(self.images.len()),
        };
        let adopted = (|| {
            // Leave a raw allocation owned by self until arena insertion succeeds.
            // Reverse at the end to preserve the request's array order.
            while let Some(&raw) = self.buffers.last() {
                let handle = gpu.buffers.insert(Owned {
                    raw,
                    last: [0; 2],
                    heap: None,
                    owner: Queue::Graphics,
                })?;
                self.buffers.pop();
                gpu.context.track_buffer(raw);
                result.buffers.push(handle);
            }
            while let Some(&raw) = self.images.last() {
                let handle = gpu.images.insert(Owned {
                    raw,
                    last: [0; 2],
                    heap: None,
                    owner: Queue::Graphics,
                })?;
                self.images.pop();
                gpu.context.track_image(raw);
                result.images.push(handle);
            }
            Ok::<_, GpuError>(())
        })();
        if let Err(error) = adopted {
            for buffer in result.buffers {
                gpu.retire(Resource::Buffer(buffer))?;
            }
            for image in result.images {
                gpu.retire(Resource::Image(image))?;
            }
            self.dispose(gpu.disposal.as_mut().expect("disposal worker alive"));
            gpu.collect()?;
            return Err(error);
        }
        result.buffers.reverse();
        result.images.reverse();
        Ok(result)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // These allocations never entered an arena or queue. This path runs on
        // the preparation worker (failed/unreceived work), or explicit shutdown.
        for image in self.images.drain(..) {
            disposal::Object::Image(image).destroy(&self.device);
        }
        for buffer in self.buffers.drain(..) {
            disposal::Object::Buffer(buffer).destroy(&self.device);
        }
    }
}
