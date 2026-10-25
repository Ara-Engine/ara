//! Writes acquired probe surfaces (`look::gi`) into the renderer: packed probe
//! records are uploaded and scattered by a compute pass (`giScatterMain`) into the
//! surface cache, meta and bordered moment tiles. A new lattice first gets an
//! empty volume, cleared on the GPU (every probe inactive and dark).
use super::*;
use crate::look::gi::{GiGpu, ProbeSurfaces, ProbeUpdate, SURFACE_TEXELS, atlas_width};
use anyhow::Context;
use ara_gpu::{RootConstants, vulkan::Command};

/// Probes per scatter dispatch (one workgroup each).
const DISPATCH_PROBES: usize = 65_535;

impl Renderer {
    /// Replaces the probe volume used for global illumination; `None` returns to
    /// sky-only ambient light. Irradiance starts dark and converges over the next
    /// frames (one more bounce per relight).
    pub fn set_probe_surfaces(&mut self, surfaces: Option<&ProbeSurfaces>) -> Result<()> {
        self.drop_probe_volume()?;
        match surfaces {
            None => Ok(()),
            Some(surfaces) => self.update_probe_surfaces(&ProbeUpdate::new(surfaces, None)?),
        }
    }

    /// Writes `update`'s probe records into the volume in place: one upload of just
    /// those records and a compute scatter. Relighting blends them in from the
    /// irradiance already converged, so stale light fades rather than flashing to
    /// sky-only; probes not written yet stay as they were. A different lattice
    /// first replaces the volume with an empty one.
    pub fn update_probe_surfaces(&mut self, update: &ProbeUpdate) -> Result<()> {
        update.validate()?;
        let same = self.look.gi.as_ref().is_some_and(|gi| {
            gi.origin == update.origin && gi.spacing == update.spacing && gi.counts == update.counts
        });
        if !same {
            self.drop_probe_volume()?;
            let volume = self.empty_volume(update)?;
            self.look.gi = Some(volume);
            // Every probe is inactive and dark until its record arrives.
            self.scatter(None, update.probes(), Access::Undefined)?;
        }
        if update.is_empty() {
            return Ok(());
        }
        // One host staging buffer, grown as needed and reused: each scatter
        // completes before the next upload.
        let needed = update.records.len();
        if self
            .look
            .probe_staging
            .is_none_or(|(_, capacity, _)| capacity < needed)
        {
            if let Some((old, _, _)) = self.look.probe_staging.take() {
                self.gpu.retire(Resource::Buffer(old))?;
                self.gpu.collect()?;
            }
            let capacity = needed.next_power_of_two();
            let buffer = self.gpu.create_buffer(
                capacity as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )?;
            let index = self.gpu.storage_buffer(buffer)?.index;
            self.look.probe_staging = Some((buffer, capacity, index));
        }
        let (staging, _, index) = self.look.probe_staging.context("probe staging")?;
        self.gpu.upload(staging, &update.records)?;
        self.scatter(Some(index), update.len(), Access::ShaderRead)?;
        if let Some(gi) = self.look.gi.as_mut() {
            gi.bias = update.bias();
        }
        Ok(())
    }

    fn drop_probe_volume(&mut self) -> Result<()> {
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
    /// bordered moment tiles and both irradiance atlases (uninitialized: clear it
    /// with `scatter(None, ..)`).
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
            access: [Access::ShaderRead; 2],
            slot: 0,
            cursor: 0,
            converge: 0,
        };
        Ok(volume)
    }

    /// Runs `giScatterMain` over `count` probes of the current volume: records from
    /// the `staging` buffer, or (without one) clearing every probe. `before` is the
    /// volume's prior access (Undefined for a new volume).
    fn scatter(&mut self, staging: Option<u32>, count: usize, before: Access) -> Result<()> {
        let gi = self.look.gi.as_ref().context("probe volume")?;
        let clear = staging.is_none();
        let mut targets = vec![
            Resource::Buffer(gi.cache),
            Resource::Buffer(gi.meta),
            Resource::Image(gi.moments),
        ];
        if clear {
            targets.extend(gi.irradiance.map(Resource::Image));
        }
        let barrier = |resource, before, after| ara_gpu::Barrier {
            resource,
            before,
            after,
            handoff: None,
        };
        let mut words = [
            0,
            staging.unwrap_or(0),
            gi.indices[0],
            gi.indices[1],
            gi.moments_storage,
            gi.atlas | (u32::from(clear) << 31),
            gi.irradiance_storage[0],
            gi.irradiance_storage[1],
        ];
        let mut recorder = self.gpu.begin(Queue::Graphics);
        recorder.barriers(targets.iter().map(|&r| barrier(r, before, Access::Storage)));
        recorder.push(Command::Pipeline(self.look.pipelines.gi_scatter));
        for first in (0..count).step_by(DISPATCH_PROBES) {
            words[0] = first as u32;
            recorder.push(Command::Constants(RootConstants { words }));
            let groups = (count - first).min(DISPATCH_PROBES) as u32;
            recorder.push(Command::Dispatch([groups, 1, 1]));
        }
        recorder.barriers(
            targets
                .iter()
                .map(|&r| barrier(r, Access::Storage, Access::ShaderRead)),
        );
        let point = self.gpu.submit(recorder.finish(), &[])?;
        self.gpu.resolve(point)?;
        Ok(())
    }
}
