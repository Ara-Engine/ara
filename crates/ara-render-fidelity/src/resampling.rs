//! Per-view direct and diffuse-path reservoirs; allocated only when selected.
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, Queue, Resource, vk, vulkan::VulkanDevice};
use ara_render::graph::State;

pub(crate) struct Reservoirs {
    pub buffers: [Buffer; 6],
    pub indices: [u32; 6],
    pub states: [State; 6],
}
impl Reservoirs {
    pub fn new(gpu: &mut VulkanDevice, size: [u32; 2]) -> Result<Self> {
        let pixels = u64::from(size[0]) * u64::from(size[1]);
        let mut allocated = Vec::new();
        for i in 0..6 {
            let result = gpu.create_buffer(
                pixels * if i < 3 { 16 } else { 48 },
                vk::BufferUsageFlags::STORAGE_BUFFER,
                false,
            );
            match result {
                Ok(buffer) => allocated.push(buffer),
                Err(error) => {
                    for buffer in allocated {
                        gpu.retire(Resource::Buffer(buffer))?;
                    }
                    gpu.collect()?;
                    return Err(error.into());
                }
            }
        }
        let buffers: [Buffer; 6] = allocated
            .try_into()
            .map_err(|_| anyhow::anyhow!("Reservoir allocation count"))?;
        let mut indices = [0; 6];
        for (i, b) in buffers.iter().enumerate() {
            indices[i] = gpu.storage_buffer(*b)?.index;
        }
        Ok(Self {
            buffers,
            indices,
            states: [State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            }; 6],
        })
    }
    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        for b in self.buffers {
            gpu.retire(Resource::Buffer(b))?;
        }
        Ok(())
    }
}
impl crate::FidelityView {
    /// Enable direct-light reservoir reuse and diffuse path reconnection at a frame
    /// boundary. Reference mode remains independently sampled. This resets history.
    pub fn set_reservoir_sampling(&mut self, gpu: &mut VulkanDevice, enabled: bool) -> Result<()> {
        anyhow::ensure!(
            !enabled || self.settings.tier == crate::Tier::Rt,
            "Reservoirs require hardware ray queries"
        );
        anyhow::ensure!(
            !enabled || matches!(self.settings.mode, crate::Mode::Hybrid),
            "Reference transport must remain independently sampled"
        );
        if enabled == self.reservoirs.is_some() {
            return Ok(());
        }
        if enabled {
            // Prewarm the optional family at explicit setup, never in render.
            let artifacts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("cooked");
            for name in [
                "reservoir_init",
                "primary_reservoir",
                "primary_reservoir_layers",
                "primary_reservoir_atmosphere",
                "reservoir_specular",
                "reservoir_specular_layers",
                "reservoir_specular_atmosphere",
                "reservoir_trace",
                "reservoir_spatial",
                "reservoir_init_layers",
                "reservoir_trace_layers",
                "reservoir_spatial_layers",
                "reservoir_init_atmosphere",
                "reservoir_trace_atmosphere",
                "reservoir_spatial_atmosphere",
                "temporal_split",
                "atrous_split",
            ] {
                if self.pipelines.contains_key(name) {
                    continue;
                }
                let cooked: ara_gpu::cooked::CookedPipeline = serde_json::from_slice(
                    &std::fs::read(artifacts.join(format!("{name}.json")))?,
                )?;
                anyhow::ensure!(
                    cooked.key == self.shader_revision,
                    "Mixed reservoir shader cohorts"
                );
                self.pipelines
                    .insert(name, gpu.create_pipeline(&cooked, &artifacts)?);
            }
            self.targets.enable_split(gpu, self.settings.render_size)?;
            self.reservoirs = Some(Reservoirs::new(gpu, self.settings.render_size)?);
        } else if let Some(old) = self.reservoirs.take() {
            old.destroy(gpu)?;
        }
        self.history = false;
        gpu.collect()?;
        Ok(())
    }
}
