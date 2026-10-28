//! What the hybrid ray queries (`renderer::rt`) read of the published content.
use super::*;

/// Ray-query instance masks (shaders/look-rt.slangh `RT_*`): opaque geometry,
/// alpha-tested cutouts (resolved in a non-opaque query) and blended or
/// transmissive surfaces, which reflection rays skip.
pub(in crate::renderer) const RAY_OPAQUE: u8 = 1;
pub(in crate::renderer) const RAY_MASKED: u8 = 2;
pub(in crate::renderer) const RAY_TRANSPARENT: u8 = 4;

/// The native content's bindless tables as hit shading reads them (`Look.rtInfo`).
#[derive(Clone, Copy)]
pub(in crate::renderer) struct RayTables {
    /// Instance rows (`InstanceRow`, after a 128-byte header): a new buffer per
    /// content generation, so it also identifies the generation.
    pub rows: Buffer,
    pub rows_index: u32,
    /// Geometry-table rows (32 bytes per LOD entry); replaced by committed poses.
    pub geometry: u32,
    /// 320-byte native materials.
    pub materials: u32,
    /// Texture residency table (native_sample.slangh).
    pub textures: u32,
    pub instances: usize,
}

impl Native {
    /// Bindless tables of the published generation.
    pub(in crate::renderer) fn ray_tables(&self) -> RayTables {
        RayTables {
            rows: self.instances,
            rows_index: self.instance_index,
            geometry: self.pool.table_index().unwrap_or(0),
            materials: self.material_index,
            textures: self.textures.binding().map_or(0, |b| b.buffer_index),
            instances: self.rows.len(),
        }
    }

    /// Shared geometry: BLAS inputs per LOD entry (`GeometryGpu::triangles`).
    pub(in crate::renderer) fn ray_geometry(&self) -> &GeometryGpu {
        &self.pool
    }

    /// Instance `index`'s current world transform, finest-LOD geometry entry (rays
    /// always see LOD 0) and ray mask.
    pub(in crate::renderer) fn ray_instance(&self, index: usize) -> (&Mat4, u32, u8) {
        let group = &self.groups.groups[self.groups.instance_group[index]];
        let mask = if group.transparent {
            RAY_TRANSPARENT
        } else if self.material_data[group.material].pbr.flags[0] == 1 {
            RAY_MASKED
        } else {
            RAY_OPAQUE
        };
        (&self.rows[index].world, group.first, mask)
    }
}
