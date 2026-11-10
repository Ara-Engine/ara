//! Synchronization2 arrays for one simultaneous portable barrier batch.
use super::recording::access;
use super::*;
use crate::sync::{BarrierRange, ResourceBarrier};
impl VulkanDevice {
    pub(super) fn validate_barrier(&self, b: ResourceBarrier) -> Result<()> {
        match b.barrier.resource {
            Resource::Buffer(h) => {
                b.range.buffer(self.buffers.get(h)?.raw.size)?;
            }
            Resource::Image(h) => {
                b.range.image(self.images.get(h)?.raw.shape)?;
            }
            Resource::AccelerationStructure(h) => {
                self.accelerations.get(h)?;
                if !matches!(b.range, BarrierRange::Whole) {
                    return Err(GpuError::Contract(
                        "Acceleration barriers cover the whole structure".into(),
                    ));
                }
            }
        }
        Ok(())
    }
    pub(super) fn encode_barriers(
        &self,
        cmd: vk::CommandBuffer,
        queue: Queue,
        barriers: impl Iterator<Item = ResourceBarrier>,
    ) -> Result<()> {
        let mut buffers = Vec::new();
        let mut images = Vec::new();
        for b in barriers {
            let range = b.range;
            let b = b.barrier;
            if b.after == Access::Undefined {
                continue;
            }
            let (mut ss, mut sa, old) = access(b.before, b.handoff.map_or(queue, |h| h.from));
            let (mut ds, mut da, new) = access(b.after, b.handoff.map_or(queue, |h| h.to));
            let mut families = (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED);
            if let Some(h) = b.handoff {
                if h.release {
                    ds = vk::PipelineStageFlags2::NONE;
                    da = vk::AccessFlags2::NONE;
                } else {
                    ss = vk::PipelineStageFlags2::NONE;
                    sa = vk::AccessFlags2::NONE;
                }
                if self.family(h.from) != self.family(h.to) {
                    families = (self.family(h.from), self.family(h.to));
                }
            }
            match b.resource {
                Resource::Image(h) => {
                    let raw = self.images.get(h)?.raw;
                    let (m, mc, l, lc) = range.image(raw.shape)?;
                    images.push(
                        vk::ImageMemoryBarrier2::default()
                            .image(raw.handle)
                            .old_layout(old)
                            .new_layout(new)
                            .src_stage_mask(ss)
                            .src_access_mask(sa)
                            .dst_stage_mask(ds)
                            .dst_access_mask(da)
                            .src_queue_family_index(families.0)
                            .dst_queue_family_index(families.1)
                            .subresource_range(
                                vk::ImageSubresourceRange::default()
                                    .aspect_mask(raw.aspect)
                                    .base_mip_level(m)
                                    .level_count(mc)
                                    .base_array_layer(l)
                                    .layer_count(lc),
                            ),
                    );
                }
                resource => {
                    let (buffer, size) = match resource {
                        Resource::Buffer(h) => {
                            let b = self.buffers.get(h)?.raw;
                            (b.handle, b.size)
                        }
                        Resource::AccelerationStructure(h) => {
                            let b = self.accelerations.get(h)?.raw.backing;
                            (b.handle, b.size)
                        }
                        _ => unreachable!("image handled above"),
                    };
                    let (offset, size) = range.buffer(size)?;
                    buffers.push(
                        vk::BufferMemoryBarrier2::default()
                            .buffer(buffer)
                            .offset(offset)
                            .size(size)
                            .src_stage_mask(ss)
                            .src_access_mask(sa)
                            .dst_stage_mask(ds)
                            .dst_access_mask(da)
                            .src_queue_family_index(families.0)
                            .dst_queue_family_index(families.1),
                    );
                }
            }
        }
        if !buffers.is_empty() || !images.is_empty() {
            // SAFETY: validated resources/ranges; backing arrays live through the native call outside rendering.
            unsafe {
                self.context.device.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default()
                        .buffer_memory_barriers(&buffers)
                        .image_memory_barriers(&images),
                );
            }
        }
        Ok(())
    }
}
