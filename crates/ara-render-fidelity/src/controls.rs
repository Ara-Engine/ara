//! Frame-boundary environment, material filtering and probe-volume controls.
use crate::{probes::ProbeVolume, scene::buffer, view::FidelityView};
use anyhow::{Result, ensure};
use ara_gpu::{Access, Device, GpuDevice, Queue, Resource, vk};
use ara_render2d::graph::State;
impl FidelityView {
    /// The directional light's angular radius in radians (half its source angle), or
    /// `None` for the Sun's: the cone sun rays sample, as the raster renderer's soft
    /// shadows (`ara_render::Renderer::set_sun_radius`). A change resets history.
    pub fn set_sun_radius(&mut self, radius: Option<f32>) -> Result<()> {
        let radius = radius.unwrap_or(crate::view::SUN_RADIUS);
        ensure!(
            radius.is_finite() && (0.0..=0.2).contains(&radius),
            "a sun radius is 0 to 0.2 radians"
        );
        if self.sun_radius != radius {
            self.sun_radius = radius;
            self.history = false;
        }
        Ok(())
    }
    /// Replace the light tables `ara_frame::scene::Light::tables` offsets address
    /// (photometric profiles; word 0 reserved), as the raster renderer's
    /// `set_light_tables`. The buffer grows only when a larger table arrives.
    pub fn set_light_tables(&mut self, gpu: &mut GpuDevice, words: &[u32]) -> Result<()> {
        if (words.len() <= 1 && self.light_tables.is_none()) || self.light_table_words == words {
            return Ok(());
        }
        if !self
            .light_tables
            .is_some_and(|(_, _, capacity)| words.len() <= capacity)
        {
            let capacity = words.len().next_power_of_two().max(4096);
            let buffer = gpu.create_buffer(
                (capacity * 4) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )?;
            let index = gpu.storage_buffer(buffer)?.index;
            if let Some((old, _, _)) = self.light_tables.replace((buffer, index, capacity)) {
                gpu.retire(Resource::Buffer(old))?;
            }
            self.history = false;
        }
        if let Some((buffer, _, _)) = self.light_tables {
            gpu.upload(buffer, bytemuck::cast_slice(words))?;
        }
        self.light_table_words = words.to_vec();
        self.history = false;
        Ok(())
    }
    /// Enable physical sky illumination) and frustum fog, or restore the extracted
    /// environment. Changing atmospheric composition resets temporal transport; the
    /// atmosphere lights alone (a moving moon) update without, as a moving sun does.
    pub fn set_atmosphere(
        &mut self,
        atmosphere: Option<crate::atmosphere::Atmosphere>,
    ) -> Result<()> {
        if let Some(a) = atmosphere {
            a.validate()?;
        }
        let composition = |a: Option<crate::atmosphere::Atmosphere>| {
            a.map(|a| crate::atmosphere::Atmosphere {
                sun: None,
                sun_is_second: false,
                other_light: None,
                ..a
            })
        };
        if composition(self.atmosphere) != composition(atmosphere) {
            self.atmosphere_dirty = true;
            self.history = false;
        }
        self.atmosphere = atmosphere;
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
    pub fn set_probe_volume(&mut self, gpu: &mut GpuDevice, volume: ProbeVolume) -> Result<()> {
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
