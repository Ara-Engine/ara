use crate::invalid;
use ara_asset::{Error, Result};
use ara_asset_format::{Builder, Kind, Lod, Mesh, Meshlet, QuantizedVertex, SkinExtra};
use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// Decoded source vertex. Position is first for meshoptimizer's strided adapter.
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
#[repr(C)]
pub struct Vertex {
    /// Mesh-local position in meters.
    pub position: [f32; 3],
    /// Unit vertex normal.
    pub normal: [f32; 3],
    /// Tangent xyz and bitangent handedness.
    pub tangent: [f32; 4],
    /// First texture coordinate set.
    pub uv0: [f32; 2],
    /// Second texture coordinate set.
    pub uv1: [f32; 2],
    /// Linear RGBA vertex color.
    pub color: [f32; 4],
    /// Skin-local joint indices.
    pub joints: [u16; 4],
    /// Skin weights, normalized while quantizing.
    pub weights: [f32; 4],
    /// Additional skin indices for influences 4–7.
    pub joints1: [u16; 4],
    /// Additional skin weights, normalized together with the primary weights.
    pub weights1: [f32; 4],
    /// Original source vertex, retained through tangent splits and fetch remapping.
    pub source_index: u32,
}
/// Deterministic geometry cook policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ara_schema::Schema)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct MeshSettings {
    /// Maximum number of LODs, including the original (1–8).
    #[ara(default = 4, range(min = 1, max = 8))]
    pub lod_count: u32,
    /// Relative simplification error ceiling.
    #[ara(default = 0.01, range(min = 0))]
    pub lod_error: f32,
    /// Cook a versioned virtual-geometry DAG for each static material primitive.
    /// Skinned and morph-deformed meshes retain the classic path.
    #[ara(default = true)]
    pub virtual_geometry: bool,
}
impl Default for MeshSettings {
    fn default() -> Self {
        Self {
            lod_count: 4,
            lod_error: 0.01,
            virtual_geometry: true,
        }
    }
}

pub(crate) fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if length > 1e-20 {
        v.map(|x| x / length)
    } else {
        [0.0, 0.0, 1.0]
    }
}
pub(crate) fn normals(vertices: &mut [Vertex], indices: &[u32]) {
    for v in vertices.iter_mut() {
        v.normal = [0.0; 3];
    }
    for tri in indices.as_chunks::<3>().0 {
        let a = vertices[tri[0] as usize].position;
        let b = vertices[tri[1] as usize].position;
        let c = vertices[tri[2] as usize].position;
        let u = std::array::from_fn::<_, 3, _>(|i| b[i] - a[i]);
        let v = std::array::from_fn::<_, 3, _>(|i| c[i] - a[i]);
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for &i in tri {
            for (a, b) in vertices[i as usize].normal.iter_mut().zip(n) {
                *a += b;
            }
        }
    }
    for v in vertices {
        v.normal = normalize(v.normal);
    }
}
/// Optimizes cache/fetch locality, quantizes attributes, builds simplification LODs
/// and meshlets. Returns the renderer's validated ARAF v1 mesh bytes.
pub fn cook_mesh(vertices: &[Vertex], indices: &[u32], settings: MeshSettings) -> Result<Vec<u8>> {
    if !cfg!(target_endian = "little") {
        return Err(Error::Invalid(
            "cooking requires a little-endian host".into(),
        ));
    }
    if vertices.is_empty()
        || indices.is_empty()
        || !indices.len().is_multiple_of(3)
        || indices.iter().any(|&i| i as usize >= vertices.len())
        || vertices.len() > u32::MAX as usize
        || !(1..=8).contains(&settings.lod_count)
        || !settings.lod_error.is_finite()
        || settings.lod_error < 0.0
    {
        return Err(invalid("invalid mesh input/settings"));
    }
    if vertices.iter().any(|v| {
        v.position
            .iter()
            .chain(v.normal.iter())
            .chain(v.tangent.iter())
            .chain(v.uv0.iter())
            .chain(v.uv1.iter())
            .chain(v.color.iter())
            .chain(v.weights.iter())
            .chain(v.weights1.iter())
            .any(|x| !x.is_finite())
            || v.uv0.iter().chain(v.uv1.iter()).any(|v| v.abs() > 65504.0)
    }) {
        return Err(invalid("nonfinite or unrepresentable vertex attribute"));
    }
    let needs_tangents = vertices.iter().all(|v| v.tangent == [0.0; 4]);
    let (prepared, source_indices) = if needs_tangents {
        let mut corners: Vec<_> = indices.iter().map(|&i| vertices[i as usize]).collect();
        mikktspace::generate_tangents(&mut TangentGeometry(&mut corners));
        let (count, remap) = meshopt::generate_vertex_remap(&corners, None);
        (
            meshopt::remap_vertex_buffer(&corners, count, &remap),
            meshopt::remap_index_buffer(None, corners.len(), &remap),
        )
    } else {
        let (count, remap) = meshopt::generate_vertex_remap(vertices, Some(indices));
        (
            meshopt::remap_vertex_buffer(vertices, count, &remap),
            meshopt::remap_index_buffer(Some(indices), vertices.len(), &remap),
        )
    };
    let mut indices = meshopt::optimize_vertex_cache(&source_indices, prepared.len());
    let vertices = meshopt::optimize_vertex_fetch(&mut indices, &prepared);
    let adapter = meshopt::VertexDataAdapter::new(
        bytemuck::cast_slice(&vertices),
        std::mem::size_of::<Vertex>(),
        0,
    )
    .map_err(invalid)?;
    let mut bounds = [f32::INFINITY; 6];
    bounds[3..].fill(f32::NEG_INFINITY);
    for v in &vertices {
        for i in 0..3 {
            bounds[i] = bounds[i].min(v.position[i]);
            bounds[i + 3] = bounds[i + 3].max(v.position[i]);
        }
    }
    let quantized: Vec<QuantizedVertex> = vertices
        .iter()
        .map(|v| {
            let mut p = [0; 4];
            for i in 0..3 {
                let span = bounds[i + 3] - bounds[i];
                p[i] = if span > 0.0 {
                    meshopt::quantize_unorm((v.position[i] - bounds[i]) / span, 16) as u16
                } else {
                    0
                };
            }
            let n = normalize(v.normal);
            let t = normalize([v.tangent[0], v.tangent[1], v.tangent[2]]);
            let weight_sum = v
                .weights
                .iter()
                .chain(&v.weights1)
                .map(|w| w.max(0.0))
                .sum::<f32>();
            QuantizedVertex {
                position: p,
                normal: [snorm(n[0]), snorm(n[1]), snorm(n[2]), 0],
                tangent: [snorm(t[0]), snorm(t[1]), snorm(t[2]), snorm(v.tangent[3])],
                uv: [v.uv0[0], v.uv0[1], v.uv1[0], v.uv1[1]]
                    .map(|x| half::f16::from_f32(x).to_bits()),
                color: v.color.map(|x| meshopt::quantize_unorm(x, 8) as u8),
                joints: v.joints,
                weights: v.weights.map(|x| {
                    if weight_sum > 0.0 {
                        meshopt::quantize_unorm(x.max(0.0) / weight_sum, 16) as u16
                    } else {
                        0
                    }
                }),
            }
        })
        .collect();
    let source_vertices: Vec<_> = vertices.iter().map(|v| v.source_index).collect();
    let extra: Vec<_> = vertices
        .iter()
        .map(|v| {
            let sum = v
                .weights
                .iter()
                .chain(&v.weights1)
                .map(|w| w.max(0.0))
                .sum::<f32>();
            SkinExtra {
                joints: v.joints1,
                weights: v.weights1.map(|w| {
                    if sum > 0.0 {
                        meshopt::quantize_unorm(w.max(0.0) / sum, 16) as u16
                    } else {
                        0
                    }
                }),
            }
        })
        .collect();
    let clusters = meshopt::build_meshlets(&indices, &adapter, 64, 124, 0.5);
    let meshlets: Vec<_> = clusters
        .meshlets
        .iter()
        .zip(clusters.iter())
        .map(|(m, view)| {
            let b = meshopt::compute_meshlet_bounds(view, &adapter);
            Meshlet {
                vertex_offset: m.vertex_offset,
                triangle_offset: m.triangle_offset,
                vertex_count: m.vertex_count,
                triangle_count: m.triangle_count,
                center: b.center,
                radius: b.radius,
                cone_axis: b.cone_axis,
                cone_cutoff: b.cone_cutoff,
            }
        })
        .collect();
    let mut lods = vec![Lod {
        first_index: 0,
        index_count: indices.len() as u32,
        error: 0.0,
        reserved: 0,
    }];
    let mut last_count = indices.len();
    let original = indices.clone();
    for level in 1..settings.lod_count {
        let mut error = 0.0;
        let target = (original.len() >> level) / 3 * 3;
        if target < 3 {
            break;
        }
        let simplified = meshopt::simplify(
            &original,
            &adapter,
            target,
            settings.lod_error,
            meshopt::SimplifyOptions::LockBorder,
            Some(&mut error),
        );
        if simplified.len() >= last_count || simplified.is_empty() {
            break;
        }
        last_count = simplified.len();
        lods.push(Lod {
            first_index: indices.len() as u32,
            index_count: simplified.len() as u32,
            error,
            reserved: 0,
        });
        indices.extend(meshopt::optimize_vertex_cache(&simplified, vertices.len()));
    }
    let mut b = Builder::new(Kind::Mesh);
    b.section(*b"BNDS", 24, bytemuck::cast_slice(&bounds).to_vec())
        .section(*b"VERT", 52, bytemuck::cast_slice(&quantized).to_vec())
        .section(*b"VSRC", 4, bytemuck::cast_slice(&source_vertices).to_vec())
        .section(*b"XSKN", 16, bytemuck::cast_slice(&extra).to_vec())
        .section(*b"INDX", 4, bytemuck::cast_slice(&indices).to_vec())
        .section(*b"LODS", 16, bytemuck::cast_slice(&lods).to_vec())
        .section(*b"MLET", 48, bytemuck::cast_slice(&meshlets).to_vec())
        .section(
            *b"MVER",
            4,
            bytemuck::cast_slice(&clusters.vertices).to_vec(),
        )
        .section(*b"MTRI", 1, clusters.triangles);
    let bytes = b.finish()?;
    Mesh::read(&bytes)?;
    Ok(bytes)
}
fn snorm(value: f32) -> i16 {
    meshopt::quantize_snorm(value, 16) as i16
}

struct TangentGeometry<'a>(&'a mut [Vertex]);
impl mikktspace::Geometry for TangentGeometry<'_> {
    fn num_faces(&self) -> usize {
        self.0.len() / 3
    }
    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }
    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.0[face * 3 + vert].position
    }
    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        self.0[face * 3 + vert].normal
    }
    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        self.0[face * 3 + vert].uv0
    }
    fn set_tangent_encoded(&mut self, tangent: [f32; 4], face: usize, vert: usize) {
        self.0[face * 3 + vert].tangent = tangent;
    }
}
