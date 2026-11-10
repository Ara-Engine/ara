//! Metal has stage-scoped barriers; subresource ranges validate portably and
//! conservatively synchronize the entire stage rather than weakening ordering.
use super::*;
use crate::sync::{BarrierRange, ResourceBarrier};
impl MetalDevice {
    pub(super) fn encode_barriers(
        &self,
        rec: &mut Recording,
        barriers: impl Iterator<Item = ResourceBarrier>,
    ) -> Result<()> {
        if matches!(rec.active, Active::Render(_)) {
            return Err(GpuError::Contract(
                "Barriers must be outside rendering scopes".into(),
            ));
        }
        let mut wait = (MTLStages::empty(), MTLStages::empty());
        let mut dependency = false;
        for b in barriers {
            match b.barrier.resource {
                crate::Resource::Buffer(h) => {
                    b.range.buffer(self.buffers.get(h)?.raw.size)?;
                }
                crate::Resource::Image(h) => {
                    b.range.image(self.images.get(h)?.raw.shape)?;
                }
                crate::Resource::AccelerationStructure(h) => {
                    self.accelerations.get(h)?;
                    if !matches!(b.range, BarrierRange::Whole) {
                        return Err(GpuError::Contract(
                            "Acceleration barriers cover the whole structure".into(),
                        ));
                    }
                }
            }
            if b.barrier.after == Access::Undefined {
                continue;
            }
            dependency = true;
            let after = stages(b.barrier.before);
            let before = stages(b.barrier.after);
            if !after.is_empty() && !before.is_empty() {
                wait.0 |= after;
                wait.1 |= before;
            }
        }
        if dependency {
            rec.wait = (rec.wait.0 | wait.0, rec.wait.1 | wait.1);
            if matches!(rec.active, Active::Compute(_)) {
                // This encoder can consume an earlier encoder's producer. It
                // also may end before the actual consumer (e.g. pass timing
                // splits transfer from dispatch), so keep the queue dependency
                // pending for the next encoder as well as ordering this one.
                let (after, waiting) = rec.wait;
                let before = waiting & COMPUTE_STAGES;
                if let Active::Compute(encoder) = &rec.active {
                    if !before.is_empty() {
                        encoder.barrierAfterQueueStages_beforeStages_visibilityOptions(
                            after,
                            before,
                            MTL4VisibilityOptions::Device,
                        );
                    }
                    encoder.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                        COMPUTE_STAGES,
                        COMPUTE_STAGES,
                        MTL4VisibilityOptions::Device,
                    );
                }
            }
        }
        Ok(())
    }
}
