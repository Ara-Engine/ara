//! Uploads acquired probe surfaces (`look::gi`) into the renderer.
use super::transfer::{upload_buffer, upload_image};
use super::*;
use crate::look::gi::{GiGpu, ProbeSurfaces, STATIC_VECTORS, atlas_width};
use ara_gpu::vulkan::Command;

/// Folds a bordered tile coordinate (-1..=size) onto the octahedral interior, so
/// border texels repeat their neighbours across the folded edge (DDGI borders).
fn fold(mut x: i32, mut y: i32, size: i32) -> usize {
    if x < 0 {
        x = -x - 1;
        y = size - 1 - y;
    } else if x >= size {
        x = 2 * size - 1 - x;
        y = size - 1 - y;
    }
    if y < 0 {
        y = -y - 1;
        x = size - 1 - x;
    } else if y >= size {
        y = 2 * size - 1 - y;
        x = size - 1 - x;
    }
    (y * size + x) as usize
}

impl Renderer {
    /// Replaces the probe volume used for global illumination; `None` returns to
    /// sky-only ambient light. Irradiance starts dark and converges over the next
    /// frames (one more bounce per relight).
    pub fn set_probe_surfaces(&mut self, surfaces: Option<&ProbeSurfaces>) -> Result<()> {
        let replacement = match surfaces {
            None => None,
            Some(surfaces) => Some(self.upload_probes(surfaces)?),
        };
        if let Some(old) = std::mem::replace(&mut self.look.gi, replacement) {
            self.gpu.retire(Resource::Buffer(old.cache))?;
            self.gpu.retire(Resource::Buffer(old.meta))?;
            for image in [old.moments, old.irradiance[0], old.irradiance[1]] {
                self.gpu.retire(Resource::Image(image))?;
            }
            self.gpu.collect()?;
        }
        Ok(())
    }

    fn upload_probes(&mut self, surfaces: &ProbeSurfaces) -> Result<GiGpu> {
        surfaces.validate()?;
        let probes = surfaces.len();
        let atlas = atlas_width(probes);
        let rows = (probes as u32).div_ceil(atlas);
        let usage = vk::BufferUsageFlags::STORAGE_BUFFER;
        let cache = upload_buffer(&mut self.gpu, bytemuck::cast_slice(&surfaces.texels), usage)?;
        let meta: Vec<[f32; 4]> = (0..probes)
            .map(|p| surfaces.statics[p * STATIC_VECTORS + 64])
            .collect();
        let meta = upload_buffer(&mut self.gpu, bytemuck::cast_slice(&meta), usage)?;
        // Static moments: bordered 10x10 tiles of the 8x8 octahedral moments (RGBA32F:
        // the upload path copies no two-channel float formats).
        let size = [atlas * 10, rows * 10];
        let mut moments = vec![[0f32; 4]; (size[0] * size[1]) as usize];
        for p in 0..probes {
            let origin = [(p as u32 % atlas) * 10, (p as u32 / atlas) * 10];
            for y in 0..10i32 {
                for x in 0..10i32 {
                    let v = surfaces.statics[p * STATIC_VECTORS + fold(x - 1, y - 1, 8)];
                    let at = (origin[1] + y as u32) * size[0] + origin[0] + x as u32;
                    moments[at as usize] = [v[0], v[1], 0., 0.];
                }
            }
        }
        let moments = upload_image(
            &mut self.gpu,
            size,
            vk::Format::R32G32B32A32_SFLOAT,
            bytemuck::cast_slice(&moments),
        )?;
        let irradiance = [
            self.dark_atlas([atlas * 8, rows * 8])?,
            self.dark_atlas([atlas * 8, rows * 8])?,
        ];
        Ok(GiGpu {
            origin: surfaces.origin,
            spacing: surfaces.spacing,
            counts: surfaces.counts,
            probes: probes as u32,
            bias: surfaces.bias(),
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
            cache,
            meta,
            moments,
            irradiance,
            access: [Access::ShaderRead; 2],
            slot: 0,
            cursor: 0,
        })
    }

    /// A zeroed RGBA16F storage atlas, left in ShaderRead.
    fn dark_atlas(&mut self, size: [u32; 2]) -> Result<Image> {
        let image = self.gpu.create_image(
            size[0],
            size[1],
            vk::Format::R16G16B16A16_SFLOAT,
            vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        let zeros = vec![0u8; (size[0] * size[1]) as usize * 8];
        let source = upload_buffer(&mut self.gpu, &zeros, vk::BufferUsageFlags::TRANSFER_SRC)?;
        let mut recorder = self.gpu.begin(Queue::Graphics);
        let barrier = |before, after| ara_gpu::Barrier {
            resource: Resource::Image(image),
            before,
            after,
            handoff: None,
        };
        recorder.barriers([barrier(Access::Undefined, Access::CopyDestination)]);
        recorder.push(Command::UploadImage {
            source,
            destination: image,
        });
        recorder.barriers([barrier(Access::CopyDestination, Access::ShaderRead)]);
        let point = self.gpu.submit(recorder.finish(), &[])?;
        self.gpu.resolve(point)?;
        self.gpu.retire(Resource::Buffer(source))?;
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn borders_fold_onto_the_octahedral_interior() {
        // Interior texels map to themselves; edges mirror with a flip.
        assert_eq!(fold(3, 2, 6), 2 * 6 + 3);
        assert_eq!(fold(-1, 0, 6), 5 * 6);
        assert_eq!(fold(6, 0, 6), 5 * 6 + 5);
        assert_eq!(fold(-1, -1, 6), 5 * 6 + 5);
    }
}
