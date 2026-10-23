//! Frame-boundary environment, material filtering and probe-volume controls.
use crate::{probes::ProbeVolume, scene::buffer, view::FidelityView};
use anyhow::{Result, ensure};
use ara_gpu::{Access, Device, Queue, Resource, vk, vulkan::VulkanDevice};
use ara_render::graph::State;
impl FidelityView {
    /// Enable physical sky illumination and frustum fog, or restore the extracted
    /// environment. Changing atmospheric composition resets temporal transport.
    pub fn set_atmosphere(
        &mut self,
        atmosphere: Option<crate::atmosphere::Atmosphere>,
    ) -> Result<()> {
        if let Some(a) = atmosphere {
            a.validate()?;
        }
        if self.atmosphere != atmosphere {
            self.atmosphere = atmosphere;
            self.atmosphere_dirty = true;
            self.history = false;
        }
        Ok(())
    }
    /// Extra mip bias in stops, in addition to log2(render/display) for TAAU.
    /// Zero retains the automatic reconstruction bias; negative values sharpen.
    pub fn set_texture_lod_bias(&mut self, bias: f32) -> Result<()> {
        ensure!(
            bias.is_finite() && (-2.0..=2.0).contains(&bias),
            "Invalid mip bias"
        );
        if self.texture_bias != bias {
            self.texture_bias = bias;
            self.history = false;
        }
        Ok(())
    }
    /// Replace the automatic probe bounds with a camera-local or authored volume.
    /// Geometry outside this volume still participates in hardware visibility.
    pub fn set_probe_volume(&mut self, gpu: &mut VulkanDevice, volume: ProbeVolume) -> Result<()> {
        volume.validate()?;
        let data = volume.empty_data();
        for i in 0..2 {
            let replacement = buffer(
                gpu,
                bytemuck::cast_slice(&data),
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            )?;
            gpu.retire(Resource::Buffer(std::mem::replace(
                &mut self.probes[i],
                replacement,
            )))?;
            self.probe_indices[i] = gpu.storage_buffer(replacement)?.index;
            self.probe_states[i] = State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            };
        }
        self.volume = volume;
        self.probe_start = 0;
        self.history = false;
        gpu.collect()?;
        Ok(())
    }
}
