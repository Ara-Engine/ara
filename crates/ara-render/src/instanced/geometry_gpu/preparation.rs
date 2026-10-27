//! Worker-owned geometry packing and allocation; publication only records copies.
use super::*;
use ara_gpu::{vk, vulkan::ResourcePreparation};
mod buffers;
pub(super) use buffers::{Buffers, SKIN0, STAGE, STATIC, TABLE, take_or_allocate};

/// Small immutable reuse metadata, captured at a content boundary. No vertices
/// or indices are copied; the previous pool remains owned by its live renderer.
pub(crate) struct GeometrySeed {
    pub(super) static_pool: Option<Buffer>,
    pub(super) layout: Vec<GeometryLayout>,
    pub(super) ranges: Vec<LodRange>,
    pub(super) lod_data: Vec<pack::PackedLod>,
}

pub(crate) struct GeometryPreparation {
    packed: pack::Packed,
    layout: Vec<GeometryLayout>,
    resident: Option<Buffer>,
    buffers: Buffers,
}

impl GeometryGpu {
    pub(crate) fn preparation_seed(&self) -> GeometrySeed {
        GeometrySeed {
            static_pool: self.static_pool,
            layout: self.layout.clone(),
            ranges: self.ranges.clone(),
            lod_data: self.lod_data.clone(),
        }
    }
}

pub(crate) fn storage_limit(gpu: &GpuDevice) -> u64 {
    u64::from(gpu.info().max_storage_buffer_range)
}

impl GeometryPreparation {
    /// Called on the content worker with an immutable checked scene.
    pub(crate) fn new(
        scene: super::super::ValidatedScene<'_>,
        limit: u64,
        previous: Option<&GeometrySeed>,
    ) -> Result<Self> {
        let scene = scene.scene();
        let mut packed = pack::Packed::new_reusing(scene, limit, previous)?;
        let layout = scene
            .geometry
            .iter()
            .zip(&packed.ranges)
            .map(|(geometry, range)| {
                let lod = &packed.lods[range.first as usize];
                GeometryLayout {
                    key: geometry.key,
                    revision: geometry.revision,
                    deforming: geometry.deforming,
                    first: lod.vertex_offset as usize / std::mem::size_of::<Vertex>(),
                    vertices: geometry.vertices.len(),
                }
            })
            .collect();
        let mut buffers = Buffers::default();
        if !packed.lods.is_empty() {
            // Indexed native draws bind the pool as their index buffer.
            let storage = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::INDEX_BUFFER;
            buffers.add(
                STATIC,
                packed.static_size,
                storage | vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::TRANSFER_SRC,
                false,
                Vec::new(),
            );
            if packed.staged_bytes > 0 {
                buffers.add(
                    STAGE,
                    packed.staged_bytes,
                    vk::BufferUsageFlags::TRANSFER_SRC,
                    true,
                    std::mem::take(&mut packed.static_bytes),
                );
            }
            if !packed.dynamic.is_empty() {
                for slot in SKIN0..SKIN0 + 3 {
                    let data = bytemuck::cast_slice(&packed.dynamic).to_vec();
                    buffers.add(slot, data.len() as u64, storage, true, data);
                }
            }
            buffers.add(
                TABLE,
                (packed.lods.len() * std::mem::size_of::<Row>()) as u64,
                storage,
                true,
                Vec::new(),
            );
        }
        Ok(Self {
            packed,
            layout,
            resident: previous.and_then(|old| old.static_pool),
            buffers,
        })
    }

    pub(crate) fn preallocate(&mut self, gpu: &mut GpuDevice) -> Result<bool> {
        let limit = storage_limit(gpu);
        ensure!(
            self.packed.static_size <= limit
                && (self.packed.dynamic.len() * std::mem::size_of::<Vertex>()) as u64 <= limit
                && (self.packed.lods.len() * std::mem::size_of::<Row>()) as u64 <= limit,
            "native prepared geometry exceeds device storage-buffer limits"
        );
        self.buffers.poll(gpu)
    }

    pub(crate) fn publish(self, gpu: &mut GpuDevice) -> Result<GeometryGpu> {
        let Self {
            packed,
            layout,
            resident,
            mut buffers,
        } = self;
        if !buffers.ready {
            buffers.destroy(gpu)?;
            anyhow::bail!("native geometry allocation is still pending");
        }
        let mut allocated = Vec::new();
        let result = GeometryGpu::upload(
            gpu,
            packed,
            false,
            resident,
            &mut allocated,
            Some(&mut buffers),
        );
        buffers.destroy(gpu)?;
        match result {
            Ok((pool, stage)) => {
                if let Some(stage) = stage {
                    gpu.retire(Resource::Buffer(stage))?;
                }
                gpu.collect()?;
                Ok(GeometryGpu { layout, ..pool })
            }
            Err(error) => {
                for resource in allocated.into_iter().rev() {
                    gpu.retire(resource)?;
                }
                gpu.collect()?;
                Err(error)
            }
        }
    }

    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        self.buffers.destroy(gpu)
    }

    pub(crate) fn source_matches(&self, previous: Option<&GeometryGpu>) -> bool {
        !self.packed.copies.iter().any(|copy| copy.resident)
            || self.resident == previous.and_then(|pool| pool.static_pool)
    }
}
