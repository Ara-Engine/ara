//! One static pool plus a separate deforming-vertex pool. LOD indices stay local.
use super::*;

pub(super) struct PackedLod {
    pub vertex_offset: u64,
    pub vertex_count: u32,
    pub index_offset: u64,
    pub index_count: u32,
    pub deforming: bool,
}

pub(super) struct Packed {
    pub static_bytes: Vec<u8>,
    pub static_size: u64,
    pub copies: Vec<Copy>,
    pub reused_geometries: usize,
    pub dynamic: Vec<Vertex>,
    pub lods: Vec<PackedLod>,
    pub ranges: Vec<LodRange>,
}

/// Static destination ranges are filled from either compact staging or the old pool.
pub(super) struct Copy {
    pub resident: bool,
    pub source: u64,
    pub destination: u64,
    pub size: u64,
}

impl Packed {
    pub fn new(scene: &Scene, limit: u64) -> Result<Self> {
        Self::new_reusing(scene, limit, None)
    }

    pub fn new_reusing(scene: &Scene, limit: u64, previous: Option<&GeometryGpu>) -> Result<Self> {
        ara_gpu::cpu_scope!("NativeGeometry.pack");
        let vertex_bytes = std::mem::size_of::<Vertex>() as u64;
        let mut static_len = 0u64;
        let mut dynamic_len = 0u64;
        let mut index_len = 0u64;
        let mut table_len = 0u64;
        for geometry in &scene.geometry {
            table_len = table_len
                .checked_add(
                    (geometry.lods.len() as u64)
                        .checked_mul(std::mem::size_of::<Row>() as u64)
                        .ok_or_else(|| anyhow::anyhow!("native geometry table overflow"))?,
                )
                .ok_or_else(|| anyhow::anyhow!("native geometry table overflow"))?;
            let bytes = (geometry.vertices.len() as u64)
                .checked_mul(vertex_bytes)
                .ok_or_else(|| anyhow::anyhow!("native vertex pool overflow"))?;
            let target = if geometry.deforming {
                &mut dynamic_len
            } else {
                &mut static_len
            };
            *target = target
                .checked_add(bytes)
                .ok_or_else(|| anyhow::anyhow!("native vertex pool overflow"))?;
            for lod in &geometry.lods {
                index_len = index_len
                    .checked_add(
                        (lod.indices.len() as u64)
                            .checked_mul(4)
                            .ok_or_else(|| anyhow::anyhow!("native index pool overflow"))?,
                    )
                    .ok_or_else(|| anyhow::anyhow!("native index pool overflow"))?;
            }
        }
        let static_total = static_len
            .checked_add(index_len)
            .ok_or_else(|| anyhow::anyhow!("native geometry pool overflow"))?;
        ensure!(
            static_total <= limit
                && dynamic_len <= limit
                && table_len <= limit
                && static_total <= u64::from(u32::MAX)
                && dynamic_len <= u64::from(u32::MAX)
                && table_len <= u64::from(u32::MAX),
            "native geometry pool exceeds device storage-buffer or shader address limits"
        );
        // Allocate staging only for new sources. Source keys include immutable
        // attributes/topology; revisions also distinguish changed rigid sources.
        let previous_keys: std::collections::BTreeMap<_, _> = previous
            .into_iter()
            .flat_map(|pool| pool.layout.iter().enumerate())
            .map(|(index, geometry)| (geometry.key, index))
            .collect();
        let reused: Vec<_> = scene
            .geometry
            .iter()
            .map(|geometry| {
                previous.and_then(|pool| {
                    let &index = previous_keys.get(&geometry.key)?;
                    let layout = &pool.layout[index];
                    let range = pool.ranges[index];
                    (pool.static_pool.is_some()
                        && layout.deforming == geometry.deforming
                        && layout.vertices == geometry.vertices.len()
                        && (geometry.deforming || layout.revision == geometry.revision)
                        && range.count as usize == geometry.lods.len()
                        && geometry.lods.iter().enumerate().all(|(i, lod)| {
                            pool.lod_data[range.first as usize + i].index_count as usize
                                == lod.indices.len()
                        }))
                    .then_some(&pool.lod_data[range.first as usize..][..range.count as usize])
                })
            })
            .collect();
        let reused_bytes: u64 = scene
            .geometry
            .iter()
            .zip(&reused)
            .filter(|(_, old)| old.is_some())
            .map(|(geometry, _)| {
                let vertices = if geometry.deforming {
                    0
                } else {
                    geometry.vertices.len() as u64 * vertex_bytes
                };
                vertices
                    + geometry
                        .lods
                        .iter()
                        .map(|lod| lod.indices.len() as u64 * 4)
                        .sum::<u64>()
            })
            .sum();
        let mut result = Self {
            static_bytes: Vec::with_capacity(usize::try_from(static_total - reused_bytes)?),
            static_size: static_total,
            copies: Vec::new(),
            reused_geometries: reused.iter().filter(|old| old.is_some()).count(),
            dynamic: Vec::with_capacity(usize::try_from(dynamic_len / vertex_bytes)?),
            lods: Vec::with_capacity(usize::try_from(
                table_len / std::mem::size_of::<Row>() as u64,
            )?),
            ranges: Vec::with_capacity(scene.geometry.len()),
        };
        let mut vertex_cursor = 0;
        let mut index_cursor = static_len;
        for (geometry, previous) in scene.geometry.iter().zip(reused) {
            let vertex_offset = if geometry.deforming {
                let offset = result.dynamic.len() as u64 * vertex_bytes;
                result.dynamic.extend_from_slice(&geometry.vertices);
                offset
            } else {
                let offset = vertex_cursor;
                let size = geometry.vertices.len() as u64 * vertex_bytes;
                if let Some(lods) = previous {
                    result.copy(true, lods[0].vertex_offset, offset, size);
                } else {
                    result.stage(offset, bytemuck::cast_slice(&geometry.vertices));
                }
                vertex_cursor += size;
                offset
            };
            let first = u32::try_from(result.lods.len())?;
            for (index, lod) in geometry.lods.iter().enumerate() {
                let size = lod.indices.len() as u64 * 4;
                result.lods.push(PackedLod {
                    vertex_offset,
                    vertex_count: u32::try_from(geometry.vertices.len())?,
                    index_offset: index_cursor,
                    index_count: u32::try_from(lod.indices.len())?,
                    deforming: geometry.deforming,
                });
                if let Some(lods) = previous {
                    result.copy(true, lods[index].index_offset, index_cursor, size);
                } else {
                    result.stage(index_cursor, bytemuck::cast_slice(&lod.indices));
                }
                index_cursor += size;
            }
            result.ranges.push(LodRange {
                first,
                count: u32::try_from(geometry.lods.len())?,
            });
        }
        Ok(result)
    }

    fn stage(&mut self, destination: u64, bytes: &[u8]) {
        let source = self.static_bytes.len() as u64;
        self.static_bytes.extend_from_slice(bytes);
        self.copy(false, source, destination, bytes.len() as u64);
    }

    fn copy(&mut self, resident: bool, source: u64, destination: u64, size: u64) {
        if let Some(last) = self.copies.last_mut()
            && last.resident == resident
            && last.source + last.size == source
            && last.destination + last.size == destination
        {
            last.size += size;
        } else {
            self.copies.push(Copy {
                resident,
                source,
                destination,
                size,
            });
        }
    }
}
