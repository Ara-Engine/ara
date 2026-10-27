//! Content replacement keeps pipelines and targets; hosts publish only completed frames.
use crate::{
    FidelityView, Mode, Tier,
    probes::ProbeVolume,
    scene::{GpuScene, buffer},
    targets::Targets,
};
use anyhow::Result;
use ara_gpu::{Access, Barrier, Buffer, Device, GpuDevice, Queue, Resource, commands::Command, vk};
use ara_render::graph::State;

impl FidelityView {
    /// Starts a reproducible capture sequence without reallocating geometry,
    /// pipelines or targets. Hosts supply a fixed jitter/RNG sequence afterward;
    /// temporal AA remains enabled. This explicit readback boundary clears DDGI,
    /// exposure and temporal accumulation rather than borrowing live lighting history.
    pub fn reset_accumulation(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        let empty = self.volume.empty_data();
        for (probe, state) in self.probes.into_iter().zip(self.probe_states) {
            overwrite(gpu, probe, bytemuck::cast_slice(&empty), state.access)?;
        }
        overwrite(
            gpu,
            self.exposure,
            bytemuck::cast_slice(&[1_f32, 0., 0., 0.]),
            self.exposure_state.access,
        )?;
        let state = State {
            access: Access::ShaderRead,
            queue: Queue::Graphics,
        };
        self.probe_states = [state; 2];
        self.exposure_state = state;
        self.probe_slot = 0;
        self.probe_start = 0;
        self.responsive_frames = 0;
        self.history = false;
        self.slot = 0;
        self.samples = 0;
        self.last_frame = None;
        self.atmosphere_dirty = true;
        Ok(())
    }

    /// Changes per-frame transport budgets without replacing geometry or pipelines.
    pub fn set_lighting_budget(
        &mut self,
        probe_ray_budget: u32,
        reflection_cutoff: f32,
    ) -> Result<()> {
        let settings = crate::Settings {
            probe_ray_budget,
            reflection_cutoff,
            ..self.settings
        };
        settings.validate()?;
        self.settings = settings;
        Ok(())
    }
    /// Advances compressed texture residency within the configured upload budget.
    pub fn refine_native_textures(
        &mut self,
        gpu: &mut GpuDevice,
    ) -> Result<ara_texture_stream::NativeStats> {
        self.scene.refine_native_textures(gpu)
    }
    /// Uploads a checked native content generation without duplicating render targets.
    /// Failed preparation retains the current scene. Hosts retain their published
    /// output until the first successful render of the new generation.
    pub fn replace_native_scene(
        &mut self,
        gpu: &mut GpuDevice,
        scene: &ara_render::instanced::Scene,
        textures: ara_texture_stream::NativeResidency,
    ) -> Result<()> {
        let next = GpuScene::new_native(gpu, scene, textures, self.settings.tier == Tier::Rt)?;
        let mut allocated = Vec::new();
        let prepared = (|| {
            let volume = ProbeVolume::automatic(next.bounds[0], next.bounds[1])?;
            let data = volume.empty_data();
            let mut probes = self.probes;
            let mut indices = self.probe_indices;
            for i in 0..2 {
                probes[i] = buffer(
                    gpu,
                    bytemuck::cast_slice(&data),
                    vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
                )?;
                allocated.push(Resource::Buffer(probes[i]));
                indices[i] = gpu.storage_buffer(probes[i])?.index;
            }
            gpu.assert_clean()?;
            Ok::<_, anyhow::Error>((volume, probes, indices))
        })();
        let (volume, probes, indices) = match prepared {
            Ok(value) => value,
            Err(error) => {
                next.destroy(gpu)?;
                for resource in allocated {
                    gpu.retire(resource)?;
                }
                gpu.collect()?;
                return Err(error);
            }
        };
        let old = std::mem::replace(&mut self.scene, next);
        let old_probes = std::mem::replace(&mut self.probes, probes);
        self.probe_indices = indices;
        self.probe_states = [State {
            // `buffer` uploaded and transitioned these initialized probes.
            access: Access::ShaderRead,
            queue: Queue::Graphics,
        }; 2];
        self.volume = volume;
        self.probe_slot = 0;
        self.probe_start = 0;
        self.responsive_frames = 7;
        self.history = false;
        self.samples = 0;
        self.last_frame = None;
        self.atmosphere_dirty = true;
        old.destroy(gpu)?;
        for probe in old_probes {
            gpu.retire(Resource::Buffer(probe))?;
        }
        gpu.collect()?;
        Ok(())
    }

    /// Resizes a host-published view without allocating two sets of storage-image
    /// descriptors. The host must keep a separate completed presentation image:
    /// failed allocation leaves this view awaiting a successful resize, while that
    /// published image remains valid. No gameplay or simulation state is touched.
    pub fn resize_unpublished(
        &mut self,
        gpu: &mut GpuDevice,
        render_size: [u32; 2],
        display_size: [u32; 2],
    ) -> Result<()> {
        let mut settings = self.settings;
        settings.render_size = render_size;
        settings.display_size = display_size;
        settings.validate()?;
        if self.targets.images.len() >= 28
            && self.settings.render_size == render_size
            && self.settings.display_size == display_size
        {
            return Ok(());
        }
        if let Some(reservoirs) = self.reservoirs.take() {
            reservoirs.destroy(gpu)?;
        }
        std::mem::replace(&mut self.targets, Targets::empty()).destroy(gpu)?;
        gpu.collect()?;
        self.targets = Targets::new(
            gpu,
            render_size,
            display_size,
            matches!(settings.mode, Mode::Reference { .. }),
        )?;
        self.settings = settings;
        self.shadow_source = crate::SunShadowSource::RayQueries;
        // Resizing allocated fresh atmosphere LUT images, even when the authored
        // atmosphere and sun did not change. They need producers before sampling.
        self.atmosphere_dirty = true;
        self.history = false;
        self.samples = 0;
        self.last_frame = None;
        Ok(())
    }
}

/// Explicit capture reset of completed device-local storage, preserving GPU memory placement.
fn overwrite(gpu: &mut GpuDevice, destination: Buffer, bytes: &[u8], before: Access) -> Result<()> {
    let stage = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let result = (|| {
        gpu.upload(stage, bytes)?;
        let mut commands = gpu.begin(Queue::Graphics);
        commands.barriers([
            Barrier {
                resource: Resource::Buffer(stage),
                before: Access::HostWrite,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(destination),
                before,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        commands.push(Command::CopyBuffer {
            source: stage,
            destination,
            size: bytes.len() as u64,
        });
        commands.barriers([Barrier {
            resource: Resource::Buffer(destination),
            before: Access::CopyDestination,
            after: Access::ShaderRead,
            handoff: None,
        }]);
        let point = gpu.submit(commands.finish(), &[])?;
        gpu.resolve(point)?;
        Ok(())
    })();
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    result
}
