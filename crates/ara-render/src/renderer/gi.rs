//! Writes acquired probe surfaces (`look::gi`) into the renderer without waiting
//! on the GPU: packed probe records are uploaded to host staging here, and the
//! next frame scatters them (`GI.Scatter`, `giScatterMain`) into the surface
//! cache, meta and bordered moment tiles ahead of its relight. A new lattice first
//! gets an empty volume, which that frame clears (`GI.Clear`: every probe
//! inactive and dark).
use super::*;
use crate::look::gi::{
    GiGpu, PROBE_RECORD, ProbeStaging, ProbeSurfaces, ProbeUpdate, SURFACE_TEXELS, atlas_width,
};
use std::ops::Range;

/// Staging buffers kept for reuse once their frame has scattered them.
const STAGING_KEPT: usize = 2;

impl Renderer {
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
        let same = self.look.gi.as_ref().is_some_and(|gi| {
            gi.origin == update.origin && gi.spacing == update.spacing && gi.counts == update.counts
        });
        if !same {
            self.drop_probe_volume()?;
            let volume = self.empty_volume(update)?;
            self.look.gi = Some(volume);
        }
        if records.is_empty() {
            return Ok(());
        }
        let bytes = &update.records[records.start * PROBE_RECORD..records.end * PROBE_RECORD];
        let staging = self.probe_staging(bytes.len())?;
        if let Err(error) = self.gpu.upload_queued(staging.buffer, bytes) {
            self.look.probe_staging.push(staging);
            return Err(error.into());
        }
        self.look.probe_writes.push((staging, records.len() as u32));
        if let Some(gi) = self.look.gi.as_mut() {
            gi.bias = update.bias();
            gi.received(records.len());
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
        let capacity = bytes.next_power_of_two();
        let buffer =
            self.gpu
                .create_buffer(capacity as u64, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        let index = self.gpu.storage_buffer(buffer)?.index;
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
            gi.fresh = false;
        }
        let written = std::mem::take(&mut self.look.probe_writes);
        let pool = &mut self.look.probe_staging;
        pool.extend(written.into_iter().map(|(staging, _)| staging));
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
            .extend(pending.into_iter().map(|(staging, _)| staging));
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
    fn empty_volume(&mut self, update: &ProbeUpdate) -> Result<GiGpu> {
        let probes = update.probes();
        let atlas = atlas_width(probes);
        let rows = (probes as u32).div_ceil(atlas);
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER;
        let cache =
            self.gpu
                .create_buffer((probes * SURFACE_TEXELS * 16) as u64, storage, false)?;
        let meta = self
            .gpu
            .create_buffer((probes * 16) as u64, storage, false)?;
        // Capture boundaries zero the irradiance with a copy (`reset_capture_history`).
        let image = |gpu: &mut VulkanDevice, size: u32, format| {
            gpu.create_image(
                atlas * size,
                rows * size,
                format,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::TRANSFER_DST,
            )
        };
        let moments = image(&mut self.gpu, 10, vk::Format::R32G32B32A32_SFLOAT)?;
        let irradiance = [
            image(&mut self.gpu, 8, vk::Format::R16G16B16A16_SFLOAT)?,
            image(&mut self.gpu, 8, vk::Format::R16G16B16A16_SFLOAT)?,
        ];
        let volume = GiGpu {
            origin: update.origin,
            spacing: update.spacing,
            counts: update.counts,
            probes: probes as u32,
            bias: update.bias(),
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
            access: [Access::Undefined; 2],
            fresh: true,
            slot: 0,
            cursor: 0,
            converge: 0,
            received: 0,
            weight: 0.,
        };
        Ok(volume)
    }
}
