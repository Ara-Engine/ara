use crate::container::{floats, u32_at};
use crate::{Container, FormatError, Kind};
use bytemuck::{Pod, Zeroable};

/// Packed 52-byte GPU vertex; positions decode using the mesh AABB.
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
#[repr(C)]
pub struct QuantizedVertex {
    /// UNORM16 xyz within bounds; w is zero.
    pub position: [u16; 4],
    /// SNORM16 xyz normal; w is zero.
    pub normal: [i16; 4],
    /// SNORM16 tangent xyz and handedness w.
    pub tangent: [i16; 4],
    /// IEEE half UV0.xy and UV1.xy.
    pub uv: [u16; 4],
    /// Linear UNORM8 RGBA vertex color.
    pub color: [u8; 4],
    /// Skin-local joint indices.
    pub joints: [u16; 4],
    /// UNORM16 weights, normalized at load or in the shader.
    pub weights: [u16; 4],
}
/// The optional second group of four skin influences (16 bytes per vertex).
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
#[repr(C)]
pub struct SkinExtra {
    /// Skin-local indices for influences 4–7.
    pub joints: [u16; 4],
    /// UNORM16 weights, normalized together with the primary four influences.
    pub weights: [u16; 4],
}
/// An index span for one simplification LOD.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct Lod {
    /// First u32 in INDX.
    pub first_index: u32,
    /// Triangle index count.
    pub index_count: u32,
    /// Relative meshoptimizer simplification error.
    pub error: f32,
    /// Reserved, zero.
    pub reserved: u32,
}
/// A meshlet with at most 64 vertices and 124 triangles.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct Meshlet {
    /// First u32 in MVER.
    pub vertex_offset: u32,
    /// First u8 in MTRI.
    pub triangle_offset: u32,
    /// Number of meshlet vertices.
    pub vertex_count: u32,
    /// Number of triangles (three local u8 indices each).
    pub triangle_count: u32,
    /// Bounding sphere center in mesh space.
    pub center: [f32; 3],
    /// Bounding sphere radius.
    pub radius: f32,
    /// Normal cone axis.
    pub cone_axis: [f32; 3],
    /// Normal cone cutoff.
    pub cone_cutoff: f32,
}
/// Borrowed, fully validated mesh payloads; no allocation or pointer casting.
#[derive(Debug, Clone, Copy)]
pub struct Mesh<'a> {
    /// xyz minimum followed by xyz maximum, as six little-endian f32 values.
    pub bounds: &'a [u8],
    /// Packed QuantizedVertex records.
    pub vertices: &'a [u8],
    /// Original source vertex for each cooked vertex, including tangent seam splits.
    pub source_vertices: &'a [u8],
    /// SkinExtra records in cooked vertex order.
    pub skin_extra: &'a [u8],
    /// Little-endian u32 triangle indices, all LODs concatenated.
    pub indices: &'a [u8],
    /// Lod records, finest first.
    pub lods: &'a [u8],
    /// Meshlet records for the finest LOD.
    pub meshlets: &'a [u8],
    /// Little-endian u32 global vertex indices.
    pub meshlet_vertices: &'a [u8],
    /// Local triangle indices as u8 triplets.
    pub meshlet_triangles: &'a [u8],
}
impl<'a> Mesh<'a> {
    /// Validates topology and all cross-section ranges before exposing bytes.
    pub fn read(bytes: &'a [u8]) -> Result<Self, FormatError> {
        let c = Container::read(bytes)?;
        if c.kind() != Kind::Mesh {
            return Err(FormatError("expected mesh"));
        }
        let m = Self {
            bounds: c.records(b"BNDS", 24)?,
            vertices: c.records(b"VERT", 52)?,
            source_vertices: c.records(b"VSRC", 4)?,
            skin_extra: c.records(b"XSKN", 16)?,
            indices: c.records(b"INDX", 4)?,
            lods: c.records(b"LODS", 16)?,
            meshlets: c.records(b"MLET", 48)?,
            meshlet_vertices: c.records(b"MVER", 4)?,
            meshlet_triangles: c.records(b"MTRI", 1)?,
        };
        if m.bounds.len() != 24 {
            return Err(FormatError("invalid bounds size"));
        }
        let mut b = [0.0; 6];
        for (i, f) in floats(m.bounds).enumerate() {
            b[i] = f;
        }
        if b.iter().any(|v| !v.is_finite()) || (0..3).any(|i| b[i] > b[i + 3]) {
            return Err(FormatError("invalid bounds"));
        }
        let n = m.vertices.len() / 52;
        if m.source_vertices.len() / 4 != n || m.skin_extra.len() / 16 != n {
            return Err(FormatError("vertex side stream count mismatch"));
        }
        for v in m.vertices.as_chunks::<52>().0 {
            for uv in v[24..32].as_chunks::<2>().0 {
                let half = u16::from_le_bytes([uv[0], uv[1]]);
                if half & 0x7c00 == 0x7c00 {
                    return Err(FormatError("nonfinite half UV"));
                }
            }
        }
        if n == 0
            || m.indices.is_empty()
            || m.lods.is_empty()
            || !(m.indices.len() / 4).is_multiple_of(3)
        {
            return Err(FormatError("empty mesh or incomplete triangle"));
        }
        for section in [m.indices, m.meshlet_vertices] {
            for i in (0..section.len()).step_by(4) {
                if u32_at(section, i)? as usize >= n {
                    return Err(FormatError("vertex index out of bounds"));
                }
            }
        }
        for l in m.lods.as_chunks::<16>().0 {
            let start = u32_at(l, 0)? as u64;
            let count = u32_at(l, 4)? as u64;
            if start + count > m.indices.len() as u64 / 4
                || !count.is_multiple_of(3)
                || !f32::from_bits(u32_at(l, 8)?).is_finite()
                || u32_at(l, 12)? != 0
            {
                return Err(FormatError("invalid LOD"));
            }
        }
        for l in m.meshlets.as_chunks::<48>().0 {
            let vo = u32_at(l, 0)? as usize;
            let to = u32_at(l, 4)? as usize;
            let vc = u32_at(l, 8)? as usize;
            let tc = u32_at(l, 12)? as usize;
            if vc == 0
                || vc > 64
                || tc == 0
                || tc > 124
                || vo as u64 + vc as u64 > m.meshlet_vertices.len() as u64 / 4
                || to as u64 + tc as u64 * 3 > m.meshlet_triangles.len() as u64
                || floats(&l[16..]).any(|x| !x.is_finite())
            {
                return Err(FormatError("invalid meshlet"));
            }
            if m.meshlet_triangles[to..to + tc * 3]
                .iter()
                .any(|&i| i as usize >= vc)
            {
                return Err(FormatError("meshlet local index out of bounds"));
            }
        }
        Ok(m)
    }
}
