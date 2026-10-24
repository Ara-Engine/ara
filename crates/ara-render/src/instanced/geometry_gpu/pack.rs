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
    pub dynamic: Vec<Vertex>,
    pub lods: Vec<PackedLod>,
    pub ranges: Vec<LodRange>,
}

impl Packed {
    pub fn new(scene: &Scene, limit: u64) -> Result<Self> {
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
        let mut static_bytes = Vec::with_capacity(usize::try_from(static_total)?);
        let mut dynamic = Vec::with_capacity(usize::try_from(dynamic_len / vertex_bytes)?);
        let mut indices = Vec::with_capacity(usize::try_from(index_len / 4)?);
        let mut lods = Vec::new();
        let mut ranges = Vec::with_capacity(scene.geometry.len());
        for geometry in &scene.geometry {
            let vertex_offset = if geometry.deforming {
                let offset = dynamic.len() as u64 * vertex_bytes;
                dynamic.extend_from_slice(&geometry.vertices);
                offset
            } else {
                let offset = static_bytes.len() as u64;
                static_bytes.extend_from_slice(bytemuck::cast_slice(&geometry.vertices));
                offset
            };
            let first = u32::try_from(lods.len())?;
            for lod in &geometry.lods {
                lods.push(PackedLod {
                    vertex_offset,
                    vertex_count: u32::try_from(geometry.vertices.len())?,
                    index_offset: static_len + indices.len() as u64 * 4,
                    index_count: u32::try_from(lod.indices.len())?,
                    deforming: geometry.deforming,
                });
                indices.extend_from_slice(&lod.indices);
            }
            ranges.push(LodRange {
                first,
                count: u32::try_from(geometry.lods.len())?,
            });
        }
        static_bytes.extend_from_slice(bytemuck::cast_slice(&indices));
        Ok(Self {
            static_bytes,
            dynamic,
            lods,
            ranges,
        })
    }
}
