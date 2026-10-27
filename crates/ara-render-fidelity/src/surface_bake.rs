//! One-time probe surface acquisition for the raster renderer's GI
//! (`ara_render::look::gi`): ray queries run here only, never per frame.
use crate::{FidelityView, Tier};
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Barrier, Device, GpuDevice, Queue, Resource, RootConstants, vk, vulkan::Command,
};
use ara_render::graph::State;
use ara_render::look::gi::{ProbeSurfaces, STATIC_VECTORS, SURFACE_TEXELS};

/// Probes per submission, bounding each dispatch well under driver timeouts.
const BATCH: usize = 2048;

impl FidelityView {
    /// Ray-traces what every probe of this view's lattice sees: relocation and
    /// classification, distance moments and averaged surface caches. Requires the
    /// RT tier and one rendered frame (the frame record supplies scene and lattice).
    pub fn bake_probe_surfaces(&mut self, gpu: &mut GpuDevice) -> Result<ProbeSurfaces> {
        ensure!(
            self.settings.tier == Tier::Rt && self.last_frame.is_some(),
            "Probe surface bakes need a rendered ray-traced view"
        );
        self.ensure_pipeline(gpu, "probe_surfaces")?;
        let pipeline = self.pipelines["probe_surfaces"];
        let probes = self.volume.len();
        let usage = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC;
        let texel_bytes = probes * SURFACE_TEXELS * 16;
        let static_bytes = probes * STATIC_VECTORS * 16;
        let cache = gpu.create_buffer(texel_bytes as u64, usage, false)?;
        let statics = gpu.create_buffer(static_bytes as u64, usage, false)?;
        let indices = [
            gpu.storage_buffer(cache)?.index,
            gpu.storage_buffer(statics)?.index,
        ];
        let barrier = |buffer, before, after| Barrier {
            resource: Resource::Buffer(buffer),
            before,
            after,
            handoff: None,
        };
        for first in (0..probes).step_by(BATCH) {
            let mut c = gpu.begin(Queue::Graphics);
            let before = if first == 0 {
                Access::Undefined
            } else {
                Access::Storage
            };
            c.barriers([
                barrier(cache, before, Access::Storage),
                barrier(statics, before, Access::Storage),
            ]);
            c.push(Command::Pipeline(pipeline));
            c.push(Command::Constants(RootConstants {
                words: [
                    self.frame_index,
                    indices[0],
                    indices[1],
                    first as u32,
                    0,
                    0,
                    0,
                    0,
                ],
            }));
            c.push(Command::Dispatch([BATCH.min(probes - first) as u32, 1, 1]));
            let point = gpu.submit(c.finish(), &[])?;
            gpu.resolve(point)?;
        }
        let storage = State {
            access: Access::Storage,
            queue: Queue::Graphics,
        };
        let texels = self.read_buffer(gpu, cache, texel_bytes, storage)?;
        let static_data = self.read_buffer(gpu, statics, static_bytes, storage)?;
        for buffer in [cache, statics] {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        gpu.collect()?;
        let words = |bytes: &[u8]| -> Vec<[u32; 4]> {
            bytes
                .as_chunks::<16>()
                .0
                .iter()
                .map(|v| {
                    std::array::from_fn(|i| {
                        u32::from_le_bytes([v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]])
                    })
                })
                .collect()
        };
        let surfaces = ProbeSurfaces {
            layout: ara_render::look::gi::ProbeLayout::single(ara_render::look::gi::ProbeLattice {
                origin: self.volume.origin.to_array(),
                spacing: self.volume.spacing.to_array(),
                counts: self.volume.counts,
            }),
            texels: words(&texels),
            statics: words(&static_data)
                .into_iter()
                .map(|v| v.map(f32::from_bits))
                .collect(),
        };
        surfaces.validate()?;
        Ok(surfaces)
    }
}
