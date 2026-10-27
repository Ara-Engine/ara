//! Allocate complete candidates off-thread; ordered copies still publish on the owner.
use super::*;
use ara_gpu::vulkan::{PreparationId, PreparedResources};
mod request;

pub(super) enum Initial {
    Waiting {
        selected: Vec<usize>,
        ticket: Option<PreparationId>,
    },
    Ready(Ready),
}

pub(super) struct Ready {
    tails: Vec<Option<(Image, Buffer)>>,
    table: Option<Buffer>,
}

impl Initial {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    pub fn take_tail(&mut self, index: usize) -> Option<(Image, Buffer)> {
        match self {
            Self::Ready(ready) => ready.tails.get_mut(index)?.take(),
            Self::Waiting { .. } => None,
        }
    }

    pub fn take_table(&mut self) -> Option<Buffer> {
        match self {
            Self::Ready(ready) => ready.table.take(),
            Self::Waiting { .. } => None,
        }
    }

    pub fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        match self {
            Self::Waiting { ticket, .. } => {
                if let Some(ticket) = ticket {
                    gpu.cancel_prepared_resources(ticket)?;
                }
            }
            Self::Ready(ready) => {
                for (image, stage) in ready.tails.into_iter().flatten() {
                    gpu.retire(Resource::Image(image))?;
                    gpu.retire(Resource::Buffer(stage))?;
                }
                if let Some(buffer) = ready.table {
                    gpu.retire(Resource::Buffer(buffer))?;
                }
            }
        }
        Ok(())
    }
}

impl NativeResidency {
    /// Poll bounded worker allocation and compressed mip staging before publishing
    /// a replacement. This never submits or waits. Keep the previous provider alive
    /// through `publish_reuse`, and call `destroy` if this candidate is superseded.
    /// Registrations and mip demand are frozen until initialization or destruction.
    pub fn preallocate(
        &mut self,
        gpu: &mut GpuDevice,
        previous: Option<&dyn ara_render::instanced::Textures>,
    ) -> Result<bool> {
        ensure!(
            self.table.is_none(),
            "native preallocation requires unpublished content"
        );
        if self.entries.is_empty() {
            return Ok(true);
        }
        if self.initial.is_some() && !self.reuse_is_valid(previous) {
            // The last-good view may refine mip tails while this worker runs.
            // Rebase before adoption if any borrowed image was replaced/retired.
            self.initial
                .take()
                .expect("preparation checked")
                .destroy(gpu)?;
            for entry in &mut self.entries {
                entry.resident = None;
            }
            self.stats.resident_bytes = 0;
        }
        if self.initial.is_none() {
            self.borrow_reusing(gpu, previous)?;
            plan::select(&self.entries, self.config.resident_bytes, &mut self.plan)?;
            let selected = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, entry)| {
                    entry
                        .resident
                        .is_none_or(|r| r.row.0[1] != self.plan[i])
                        .then_some(i)
                })
                .collect();
            self.initial = Some(Initial::Waiting {
                selected,
                ticket: None,
            });
        }
        let Some(Initial::Waiting { selected, ticket }) = &mut self.initial else {
            return Ok(true);
        };
        if ticket.is_none() {
            let request = request::build(&self.entries, &self.plan, selected)?;
            *ticket = gpu.prepare_resources(|| request)?;
            return Ok(false);
        }
        // Poll consumes a failed/completed ticket. Keep a pending ticket only when
        // the worker has not replied, so cancellation never sees a stale handle.
        let id = ticket.take().expect("preparation ticket checked");
        let Some(resources) = gpu.poll_prepared_resources(id)? else {
            *ticket = Some(id);
            return Ok(false);
        };
        let ready = Ready::new(gpu, self.entries.len(), selected, resources)?;
        self.initial = Some(Initial::Ready(ready));
        Ok(true)
    }
}

impl Ready {
    fn new(
        gpu: &mut GpuDevice,
        entries: usize,
        selected: &[usize],
        mut resources: PreparedResources,
    ) -> Result<Self> {
        if resources.images.len() != selected.len() || resources.buffers.len() != selected.len() + 1
        {
            for image in resources.images {
                gpu.retire(Resource::Image(image))?;
            }
            for buffer in resources.buffers {
                gpu.retire(Resource::Buffer(buffer))?;
            }
            anyhow::bail!("native preparation resource count mismatch");
        }
        let table = resources.buffers.pop();
        let mut tails = vec![None; entries];
        for ((&index, image), stage) in selected.iter().zip(resources.images).zip(resources.buffers)
        {
            tails[index] = Some((image, stage));
        }
        Ok(Self { tails, table })
    }
}
