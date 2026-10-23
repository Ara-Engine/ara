//! Worker-side asset mesh extraction for the GPU foliage instancer.
use anyhow::{Result, ensure};
use ara_asset_format::{Mesh, QuantizedVertex};

/// Vertex-pulling ABI for authored foliage meshes, 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FoliageVertex {
    /// Mesh-local position in metres, with the root at ground level.
    pub position: [f32; 3],
    /// Mesh-local unit normal.
    pub normal: [f32; 3],
    /// Texture coordinates for colour/alpha.
    pub uv: [f32; 2],
}

/// One species/LOD mesh. Far LODs can be alpha-tested baked impostor cards.
pub struct FoliageMesh {
    /// Triangle-expanded vertices; generated offline from cooked indexed assets.
    pub vertices: Vec<FoliageVertex>,
    /// Index into FoliageGeometry::textures, or None for solid colour.
    pub texture: Option<usize>,
    /// Linear albedo multiplier and alpha cutoff.
    pub color_cutoff: [f32; 4],
    /// Rotate this upright impostor card toward the camera.
    pub billboard: bool,
}
impl FoliageMesh {
    /// Expands a validated ara-asset-format LOD on a worker. Meshes are static;
    /// skinning and authoritative gameplay trees belong to their own ECS systems.
    pub fn from_cooked(bytes: &[u8], lod: usize, texture: Option<usize>) -> Result<Self> {
        let mesh = Mesh::read(bytes)?;
        let bounds: Vec<_> = mesh
            .bounds
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let count = mesh.lods.len() / 16;
        ensure!(lod < count, "foliage LOD absent in cooked mesh");
        let record = &mesh.lods[lod * 16..lod * 16 + 16];
        let first = u32::from_le_bytes(record[..4].try_into()?) as usize;
        let length = u32::from_le_bytes(record[4..8].try_into()?) as usize;
        let mut vertices = Vec::with_capacity(length);
        for word in mesh.indices[first * 4..(first + length) * 4]
            .as_chunks::<4>()
            .0
        {
            let index = u32::from_le_bytes(*word) as usize;
            let v = bytemuck::pod_read_unaligned::<QuantizedVertex>(
                &mesh.vertices[index * 52..index * 52 + 52],
            );
            let position = std::array::from_fn(|i| {
                bounds[i] + (bounds[i + 3] - bounds[i]) * f32::from(v.position[i]) / 65535.
            });
            let normal = glam::Vec3::from_array(std::array::from_fn(|i| {
                (f32::from(v.normal[i]) / 32767.).max(-1.)
            }))
            .normalize_or_zero()
            .to_array();
            let uv = [
                half::f16::from_bits(v.uv[0]).to_f32(),
                half::f16::from_bits(v.uv[1]).to_f32(),
            ];
            vertices.push(FoliageVertex {
                position,
                normal,
                uv,
            });
        }
        Ok(Self {
            vertices,
            texture,
            color_cutoff: [1., 1., 1., 0.5],
            billboard: false,
        })
    }
}

/// Species-major, LOD-minor mesh catalogue (grass/tree/rock, three LODs each).
/// Rule.asset resolution supplies these meshes before asynchronous scene publication.
pub struct FoliageGeometry {
    /// Nine meshes; the third per species is normally a baked impostor card.
    pub meshes: [FoliageMesh; 9],
    /// RGBA8 colour/alpha textures. sRGB decoding is performed exactly once.
    pub textures: Vec<ara_render::scene::Texture>,
}
impl FoliageGeometry {
    /// Updates extracted culling bounds before upload, including vertex-wind sway.
    pub fn apply_bounds(&self, instances: &mut [ara_foliage::Instance]) {
        let radii: [f32; 3] = std::array::from_fn(|species| {
            self.meshes[species * 3..species * 3 + 3]
                .iter()
                .flat_map(|m| &m.vertices)
                .map(|v| glam::Vec3::from_array(v.position).length())
                .fold(0f32, f32::max)
        });
        for instance in instances {
            if let Some(radius) = radii.get(instance.identity[2] as usize) {
                instance.rotation_wind[3] = radius * instance.position_scale[3] * 1.25;
            }
        }
    }
    pub(crate) fn pack(&self, slots: &[u32]) -> Result<Vec<u8>> {
        let mut headers = Vec::<[u32; 8]>::new();
        let mut vertices = Vec::new();
        for mesh in &self.meshes {
            ensure!(
                !mesh.vertices.is_empty()
                    && mesh.vertices.len() % 3 == 0
                    && mesh.vertices.len() <= 1_000_000,
                "invalid foliage mesh triangle count"
            );
            ensure!(
                mesh.color_cutoff.iter().all(|v| v.is_finite())
                    && (0.0..=1.0).contains(&mesh.color_cutoff[3]),
                "invalid foliage colour/cutoff"
            );
            ensure!(
                mesh.vertices.iter().all(|v| v
                    .position
                    .iter()
                    .chain(&v.normal)
                    .chain(&v.uv)
                    .all(|v| v.is_finite())),
                "nonfinite foliage vertex"
            );
            ensure!(mesh.vertices.iter().all(|v|glam::Vec3::from_array(v.normal).length_squared()>0.01),"foliage vertices need valid normals");
            let texture = match mesh.texture {
                Some(i) => *slots
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("missing foliage texture"))?,
                None => u32::MAX,
            };
            let flags = u32::from(mesh.billboard)
                | (u32::from(mesh.texture.is_some_and(|i| self.textures[i].srgb)) * 2);
            headers.push([
                vertices.len() as u32,
                mesh.vertices.len() as u32,
                texture,
                flags,
                mesh.color_cutoff[0].to_bits(),
                mesh.color_cutoff[1].to_bits(),
                mesh.color_cutoff[2].to_bits(),
                mesh.color_cutoff[3].to_bits(),
            ]);
            vertices.extend_from_slice(&mesh.vertices);
        }
        let mut bytes = bytemuck::cast_slice(&headers).to_vec();
        bytes.extend_from_slice(bytemuck::cast_slice(&vertices));
        Ok(bytes)
    }
}
