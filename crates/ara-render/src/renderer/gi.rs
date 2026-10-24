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

    /// Adopts newer surfaces of the same lattice (an edit or a new content
    /// generation) without restarting convergence: the relit irradiance carries
    /// over and relighting blends the new surfaces in, so stale light fades rather
    /// than flashing to sky-only. A different lattice replaces the volume.
    pub fn update_probe_surfaces(&mut self, surfaces: &ProbeSurfaces) -> Result<()> {
        let same = self.look.gi.as_ref().is_some_and(|gi| {
            gi.origin == surfaces.origin
                && gi.spacing == surfaces.spacing
                && gi.counts == surfaces.counts
        });
        if !same {
            return self.set_probe_surfaces(Some(surfaces));
        }
        let (cache, meta, moments) = self.upload_surfaces(surfaces)?;
        let indices = [
            self.gpu.storage_buffer(cache)?.index,
            self.gpu.storage_buffer(meta)?.index,
            self.gpu.sampled_image(moments, false)?.index,
        ];
        let Some(gi) = self.look.gi.as_mut() else {
            return Ok(());
        };
        let old = (
            std::mem::replace(&mut gi.cache, cache),
            std::mem::replace(&mut gi.meta, meta),
            std::mem::replace(&mut gi.moments, moments),
        );
        gi.indices = indices;
        gi.bias = surfaces.bias();
        self.gpu.retire(Resource::Buffer(old.0))?;
        self.gpu.retire(Resource::Buffer(old.1))?;
        self.gpu.retire(Resource::Image(old.2))?;
        self.gpu.collect()?;
        Ok(())
    }

    /// Uploads the static per-probe data: surface cache, meta (relocation and
    /// activity) and the bordered distance-moment atlas.
    fn upload_surfaces(&mut self, surfaces: &ProbeSurfaces) -> Result<(Buffer, Buffer, Image)> {
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
        Ok((cache, meta, moments))
    }

    fn upload_probes(&mut self, surfaces: &ProbeSurfaces) -> Result<GiGpu> {
        let (cache, meta, moments) = self.upload_surfaces(surfaces)?;
        let probes = surfaces.len();
        let atlas = atlas_width(probes);
        let rows = (probes as u32).div_ceil(atlas);
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
