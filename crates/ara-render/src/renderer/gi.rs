//! Writes acquired probe surfaces (`look::gi`) into the renderer without waiting
//! on the GPU: packed probe records are uploaded to host staging here, and the
//! next frame scatters them (`GI.Scatter`, `giScatterMain`) into the surface
//! cache, meta and bordered moment tiles ahead of its relight. A new lattice first
//! gets an empty volume. Startup reserves and clears it in bounded slices behind
//! loading; live replacement clears it in its first frame (`GI.Clear`).
use super::*;
use crate::look::gi::{
    GiGpu, PROBE_RECORD, ProbeLayout, ProbeStaging, ProbeSurfaces, ProbeUpdate, SURFACE_TEXELS,
    atlas_width,
};
use anyhow::Context;
use ara_gpu::commands::Command;
use std::ops::Range;

/// Staging buffers kept for reuse once their frame has scattered them.
const STAGING_KEPT: usize = 2;
#[cfg(test)]
mod tests;

impl Renderer {
    /// Reserve the initial lattice and two prefaulted staging buffers before
    /// surface acquisition finishes. Call on the loading worker at a completed
    /// boundary, then present until this returns true. Clear work is bounded by
    /// `records_per_frame`; ordinary first delivery only uploads/scatters records.
    /// An existing different lattice is retained and rejected here; live content
    /// replacement continues through `update_probe_records`.
    pub fn preallocate_probe_volume(
        &mut self,
        layout: ProbeLayout,
        records_per_frame: usize,
    ) -> Result<bool> {
        ara_gpu::cpu_scope!("Raster.reserve_probe_volume");
        layout.validate()?;
        ensure!(records_per_frame > 0, "Probe preparation budget is zero");
        ensure!(
            !self.gpu.deferring_completion(),
            "Preallocate probes at a completed loading boundary"
        );
        if let Some(gi) = &self.look.gi {
            ensure!(
                gi.layout == layout,
                "Initial probe reservation would replace a live lattice"
            );
        } else {
            let mut volume = self.empty_volume(layout)?;
            volume.clear_budget = records_per_frame.min(layout.probe_count()) as u32;
            self.look.gi = Some(volume);
        }
        let records = records_per_frame.min(layout.probe_count());
        let bytes = records
            .checked_mul(PROBE_RECORD)
            .context("Probe staging size overflow")?;
        let have = self
            .look
            .probe_staging
            .iter()
            .filter(|s| s.capacity >= bytes)
            .count();
        if have < STAGING_KEPT {
            let capacity = bytes
                .checked_next_power_of_two()
                .context("Probe staging capacity overflow")?;
            let zeros = vec![0u8; capacity];
            for _ in have..STAGING_KEPT {
                let staging = self.new_probe_staging(bytes)?;
                if let Err(error) = self.gpu.upload(staging.buffer, &zeros) {
                    self.gpu.retire(Resource::Buffer(staging.buffer))?;
                    return Err(error.into());
                }
                self.look.probe_staging.push(staging);
            }
        }
        Ok(self
            .look
            .gi
            .as_ref()
            .is_some_and(|gi| !gi.fresh && gi.relit_once))
    }

    /// A loading reservation still clearing bounded slices. Its records must
    /// remain queued until complete, so later clear slices cannot erase them.
    pub fn probe_preparation_pending(&self) -> bool {
        self.look.gi.as_ref().is_some_and(|gi| gi.fresh)
    }

    /// Finish an incomplete loading clear at an explicit capture boundary. Live
    /// frames never call this: they keep their staged records queued and clear
    /// in slices. The acquired surface cache is not reset by this operation.
    pub fn complete_probe_preparation(&mut self) -> Result<()> {
        if !self.probe_preparation_pending() {
            return Ok(());
        }
        ensure!(
            !self.gpu.deferring_completion(),
            "Capture must drain its pending frame before clearing probes"
        );
        let gi = self.look.gi.as_mut().context("pending probe volume")?;
        gi.clear_budget = gi.probes;
        let resources = [
            (Resource::Buffer(gi.cache), gi.surface_access[0]),
            (Resource::Buffer(gi.meta), gi.surface_access[1]),
            (Resource::Image(gi.moments), gi.surface_access[2]),
            (Resource::Image(gi.irradiance[0]), gi.access[0]),
            (Resource::Image(gi.irradiance[1]), gi.access[1]),
        ];
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.barriers(resources.map(|(resource, before)| ara_gpu::Barrier {
            resource,
            before,
            after: Access::Storage,
            handoff: None,
        }));
        commands.push(Command::BeginPass("GI.Clear".into()));
        self.look.record_probe_clear(&mut commands);
        commands.push(Command::EndPass);
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.gpu.resolve(point)?;
        let gi = self.look.gi.as_mut().context("cleared probe volume")?;
        gi.surface_access = [Access::Storage; 3];
        gi.access = [Access::Storage; 2];
        gi.cleared();
        self.gpu.assert_clean()?;
        Ok(())
    }
    /// Marks the coming frames as unseen: hosts that render frames nobody sees yet
    /// (an embedded Play entry publishes nothing until it is lit) set `true`, and
    /// probe GI then converges its whole volume per bounce and shows without its
    /// fade, so readiness arrives in a few frames. `false` (shown frames) returns
    /// to the budgeted relight and fades.
    pub fn set_unseen(&mut self, unseen: bool) {
        self.look.unseen = unseen;
        if let Some(gi) = &mut self.look.gi {
            gi.unseen = unseen;
        }
    }
    /// Replaces the probe volume used for global illumination; `None` returns to
    /// sky-only ambient light. The new volume relights out of sight for
    /// `CONVERGE_FRAMES`, then fades into shading.
    pub fn set_probe_surfaces(&mut self, surfaces: Option<&ProbeSurfaces>) -> Result<()> {
        self.drop_probe_volume()?;
        match surfaces {
            None => Ok(()),
            Some(surfaces) => self.update_probe_surfaces(&ProbeUpdate::new(surfaces, None)?),
        }
    }

    /// Writes all of `update`'s records ([`Self::update_probe_records`]).
    pub fn update_probe_surfaces(&mut self, update: &ProbeUpdate) -> Result<()> {
        self.update_probe_records(update, 0..update.len())
    }

    /// Writes `records` (a range of `update`'s records) into the volume: they are
    /// uploaded now and scattered by the next frame before it relights, so nothing
    /// waits on the GPU. Relighting blends them in from the irradiance already
    /// converged, so stale light fades rather than flashing to sky-only; probes
    /// not written yet stay as they were. A different lattice first replaces the
    /// volume with an empty one, kept out of shading (the ambient stays sky-only)
    /// until all its records land and it converges. Live hosts apply a large
    /// update a frame's chunk at a time.
    pub fn update_probe_records(
        &mut self,
        update: &ProbeUpdate,
        records: Range<usize>,
    ) -> Result<()> {
        update.validate()?;
        ensure!(
            records.start <= records.end && records.end <= update.len(),
            "probe records {records:?} outside the update's {}",
            update.len()
        );
        let same = self
            .look
            .gi
            .as_ref()
            .is_some_and(|gi| gi.layout == update.layout);
        if !same {
            let volume = self.empty_volume(update.layout())?;
            self.drop_probe_volume()?;
            self.look.gi = Some(volume);
        }
        if records.is_empty() {
            return Ok(());
        }
        ensure!(
            !self.look.gi.as_ref().is_some_and(GiGpu::clearing_in_slices),
            "Probe reservation must clear before record delivery"
        );
        let bytes = &update.records[records.start * PROBE_RECORD..records.end * PROBE_RECORD];
        let staging = self.probe_staging(bytes.len())?;
        if let Err(error) = self.gpu.upload_queued(staging.buffer, bytes) {
            self.look.probe_staging.push(staging);
            return Err(error.into());
        }
        // Writes of a pool's finer lattices alone may land after readiness.
        let deferred = self.look.gi.as_ref().is_some_and(|gi| {
            gi.deferred > 0 && update.ids(records.clone()).all(|id| id < gi.deferred)
        });
        self.look
            .probe_writes
            .push((staging, records.len() as u32, deferred));
        if let Some(gi) = self.look.gi.as_mut() {
            gi.received(update.ids(records));
        }
        Ok(())
    }

    /// A staging buffer of at least `bytes`: the smallest that fits from the pool,
    /// else a new one.
    fn probe_staging(&mut self, bytes: usize) -> Result<ProbeStaging> {
        let pool = &mut self.look.probe_staging;
        if let Some(i) = (0..pool.len())
            .filter(|&i| pool[i].capacity >= bytes)
            .min_by_key(|&i| pool[i].capacity)
        {
            return Ok(pool.swap_remove(i));
        }
        self.new_probe_staging(bytes)
    }

    fn new_probe_staging(&mut self, bytes: usize) -> Result<ProbeStaging> {
        ara_gpu::cpu_scope!("Raster.allocate_probe_staging");
        let capacity = bytes.next_power_of_two();
        let buffer =
            self.gpu
                .create_buffer(capacity as u64, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        let index = match self.gpu.storage_buffer(buffer) {
            Ok(index) => index.index,
            Err(error) => {
                self.gpu.retire(Resource::Buffer(buffer))?;
                return Err(error.into());
            }
        };
        Ok(ProbeStaging {
            buffer,
            capacity,
            index,
        })
    }

    /// After a frame scattered the queued writes: their staging returns to the
    /// pool (the largest [`STAGING_KEPT`] stay) and a new volume counts as cleared.
    pub(super) fn probe_writes_scattered(&mut self) -> Result<()> {
        if let Some(gi) = &mut self.look.gi {
            gi.cleared();
        }
        let written = std::mem::take(&mut self.look.probe_writes);
        let pool = &mut self.look.probe_staging;
        pool.extend(written.into_iter().map(|(staging, _, _)| staging));
        pool.sort_unstable_by_key(|s| std::cmp::Reverse(s.capacity));
        for extra in pool.split_off(pool.len().min(STAGING_KEPT)) {
            self.gpu.retire(Resource::Buffer(extra.buffer))?;
        }
        Ok(())
    }

    fn drop_probe_volume(&mut self) -> Result<()> {
        // Writes queued for the old volume are dropped with it.
        let pending = std::mem::take(&mut self.look.probe_writes);
        self.look
            .probe_staging
            .extend(pending.into_iter().map(|(staging, _, _)| staging));
        if let Some(old) = self.look.gi.take() {
            self.gpu.retire(Resource::Buffer(old.cache))?;
            self.gpu.retire(Resource::Buffer(old.meta))?;
            for image in [old.moments, old.irradiance[0], old.irradiance[1]] {
                self.gpu.retire(Resource::Image(image))?;
            }
            self.gpu.collect()?;
        }
        Ok(())
    }

    /// A volume for `update`'s lattice: storage for the surface cache, meta,
    /// bordered moment tiles and both irradiance atlases, uninitialized until the
    /// next frame clears it (`fresh`).
    fn empty_volume(&mut self, layout: ProbeLayout) -> Result<GiGpu> {
        ara_gpu::cpu_scope!("Raster.allocate_probe_volume");
        layout.validate()?;
        let mut allocated = Vec::new();
        let result = self.allocate_probe_volume(layout, &mut allocated);
        if result.is_err() {
            for resource in allocated.into_iter().rev() {
                self.gpu.retire(resource)?;
            }
            self.gpu.collect()?;
        }
        result
    }

    fn allocate_probe_volume(
        &mut self,
        layout: ProbeLayout,
        allocated: &mut Vec<Resource>,
    ) -> Result<GiGpu> {
        let probes = layout.probe_count();
        let atlas = atlas_width(probes);
        let rows = (probes as u32).div_ceil(atlas);
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER;
        let cache =
            self.gpu
                .create_buffer((probes * SURFACE_TEXELS * 16) as u64, storage, false)?;
        allocated.push(Resource::Buffer(cache));
        let meta = self
            .gpu
            .create_buffer((probes * 16) as u64, storage, false)?;
        allocated.push(Resource::Buffer(meta));
        // Capture boundaries zero the irradiance with a copy (`reset_capture_history`);
        // tests read it back.
        let mut image = |gpu: &mut GpuDevice, size: u32, format| {
            let image = gpu.create_image(
                atlas * size,
                rows * size,
                format,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )?;
            allocated.push(Resource::Image(image));
            Ok::<_, ara_gpu::GpuError>(image)
        };
        let moments = image(&mut self.gpu, 10, vk::Format::R32G32B32A32_SFLOAT)?;
        let irradiance = [
            image(&mut self.gpu, 8, vk::Format::R16G16B16A16_SFLOAT)?,
            image(&mut self.gpu, 8, vk::Format::R16G16B16A16_SFLOAT)?,
        ];
        let volume = GiGpu {
            layout,
            probes: probes as u32,
            atlas,
            indices: [
                self.gpu.storage_buffer(cache)?.index,
                self.gpu.storage_buffer(meta)?.index,
                self.gpu.sampled_image(moments, false)?.index,
            ],
            irradiance_sampled: [
                self.gpu.sampled_image(irradiance[0], false)?.index,
                self.gpu.sampled_image(irradiance[1], false)?.index,
            ],
            irradiance_storage: [
                self.gpu.storage_image(irradiance[0])?.index,
                self.gpu.storage_image(irradiance[1])?.index,
            ],
            moments_storage: self.gpu.storage_image(moments)?.index,
            cache,
            meta,
            moments,
            irradiance,
            surface_access: [Access::Undefined; 3],
            access: [Access::Undefined; 2],
            fresh: true,
            clear_cursor: 0,
            clear_budget: probes as u32,
            slot: 0,
            cursor: 0,
            relit: (0, 0),
            converge: 0,
            converge_progress: 0,
            capture_convergence: false,
            unseen: self.look.unseen,
            relit_once: false,
            received: 0,
            weight: 0.,
            // A pool's finer lattices may land after its coarsest (the last).
            deferred: layout.bases()[layout.lattices().len() - 1] as u32,
            deferred_received: 0,
            deferred_started: false,
            deferred_converge: 0,
            deferred_progress: 0,
            deferred_weight: 0.,
        };
        Ok(volume)
    }
}
