//! Uploads acquired probe surfaces (`look::gi`) into the renderer.
use super::transfer::upload_buffer;
use super::*;
use crate::look::gi::{GiGpu, IRRADIANCE_TEXELS, ProbeSurfaces};

impl Renderer {
    /// Replaces the probe volume used for global illumination; `None` returns to
    /// sky-only ambient light. Irradiance starts dark and converges over the next
    /// frames (one more bounce per frame).
    pub fn set_probe_surfaces(&mut self, surfaces: Option<&ProbeSurfaces>) -> Result<()> {
        let replacement = match surfaces {
            None => None,
            Some(surfaces) => {
                surfaces.validate()?;
                let usage = vk::BufferUsageFlags::STORAGE_BUFFER;
                let cache =
                    upload_buffer(&mut self.gpu, bytemuck::cast_slice(&surfaces.texels), usage)?;
                let statics = upload_buffer(
                    &mut self.gpu,
                    bytemuck::cast_slice(&surfaces.statics),
                    usage,
                )?;
                let dark = vec![0u8; surfaces.len() * IRRADIANCE_TEXELS * 16];
                let irradiance = [
                    upload_buffer(&mut self.gpu, &dark, usage)?,
                    upload_buffer(&mut self.gpu, &dark, usage)?,
                ];
                let indices = [
                    self.gpu.storage_buffer(cache)?.index,
                    self.gpu.storage_buffer(statics)?.index,
                    self.gpu.storage_buffer(irradiance[0])?.index,
                    self.gpu.storage_buffer(irradiance[1])?.index,
                ];
                Some(GiGpu {
                    origin: surfaces.origin,
                    spacing: surfaces.spacing,
                    counts: surfaces.counts,
                    probes: surfaces.len() as u32,
                    bias: surfaces.bias(),
                    cache,
                    statics,
                    irradiance,
                    indices,
                    access: [Access::ShaderRead; 2],
                    slot: 0,
                    cursor: 0,
                })
            }
        };
        if let Some(old) = std::mem::replace(&mut self.look.gi, replacement) {
            for buffer in [old.cache, old.statics, old.irradiance[0], old.irradiance[1]] {
                self.gpu.retire(Resource::Buffer(buffer))?;
            }
            self.gpu.collect()?;
        }
        Ok(())
    }
}
