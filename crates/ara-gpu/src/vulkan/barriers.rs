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
            let state = lower(
                b.before,
                b.after,
                b.handoff,
                queue,
                [self.family(Queue::Graphics), self.family(Queue::Compute)],
            );
            let (ss, sa, old) = state.source;
            let (ds, da, new) = state.destination;
            let families = state.families;
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

// Pure lowering keeps the exact native scopes/layouts testable without a driver.
struct BarrierState {
    source: (vk::PipelineStageFlags2, vk::AccessFlags2, vk::ImageLayout),
    destination: (vk::PipelineStageFlags2, vk::AccessFlags2, vk::ImageLayout),
    families: (u32, u32),
}
fn lower(
    before: Access,
    after: Access,
    handoff: Option<crate::Handoff>,
    queue: Queue,
    queue_families: [u32; 2],
) -> BarrierState {
    let (mut ss, mut sa, mut old) = access(before, handoff.map_or(queue, |h| h.from));
    let (mut ds, mut da, new) = access(after, handoff.map_or(queue, |h| h.to));
    let mut families = (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED);
    if let Some(h) = handoff {
        if h.release {
            ds = vk::PipelineStageFlags2::NONE;
            da = vk::AccessFlags2::NONE;
        } else {
            ss = vk::PipelineStageFlags2::NONE;
            sa = vk::AccessFlags2::NONE;
        }
        if queue_families[h.from.index()] != queue_families[h.to.index()] {
            families = (queue_families[h.from.index()], queue_families[h.to.index()]);
        } else if !h.release {
            // With ignored family indices these are ordinary barriers: the release
            // already changed the layout. Keep the acquire's destination scopes.
            old = new;
        }
    }
    BarrierState {
        source: (ss, sa, old),
        destination: (ds, da, new),
        families,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Handoff;
    #[test]
    fn regression_shared_family_image_handoff_transitions_once() {
        for (from, to) in [
            (Queue::Compute, Queue::Graphics),
            (Queue::Graphics, Queue::Compute),
        ] {
            let release = lower(
                Access::Storage,
                Access::ShaderRead,
                Some(Handoff {
                    from,
                    to,
                    release: true,
                }),
                from,
                [3, 3],
            );
            let acquire = lower(
                Access::Storage,
                Access::ShaderRead,
                Some(Handoff {
                    from,
                    to,
                    release: false,
                }),
                to,
                [3, 3],
            );
            assert_eq!(release.source, access(Access::Storage, from));
            assert_eq!(
                release.destination,
                (
                    vk::PipelineStageFlags2::NONE,
                    vk::AccessFlags2::NONE,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                )
            );
            assert_eq!(
                acquire.source,
                (
                    vk::PipelineStageFlags2::NONE,
                    vk::AccessFlags2::NONE,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                )
            );
            assert_eq!(acquire.destination, access(Access::ShaderRead, to));
            assert_eq!(
                release.families,
                (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED)
            );
            assert_eq!(acquire.families, release.families);
        }
    }
    #[test]
    fn distinct_family_image_handoff_keeps_matching_ownership_layouts() {
        let release = lower(
            Access::Storage,
            Access::ShaderRead,
            Some(Handoff {
                from: Queue::Compute,
                to: Queue::Graphics,
                release: true,
            }),
            Queue::Compute,
            [2, 5],
        );
        let acquire = lower(
            Access::Storage,
            Access::ShaderRead,
            Some(Handoff {
                from: Queue::Compute,
                to: Queue::Graphics,
                release: false,
            }),
            Queue::Graphics,
            [2, 5],
        );
        assert_eq!(release.source.2, vk::ImageLayout::GENERAL);
        assert_eq!(acquire.source.2, release.source.2);
        assert_eq!(acquire.destination.2, release.destination.2);
        assert_eq!(release.families, (5, 2));
        assert_eq!(acquire.families, (5, 2));
    }
}
