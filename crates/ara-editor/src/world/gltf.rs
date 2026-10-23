//! Temporary example-style glTF geometry loader; ara-asset will own cooked assets.
use ara_render::scene::{Mesh, Vertex};
use glam::{Mat4, Vec3};
use std::path::Path;

pub(crate) fn load(
    path: &Path,
    entity: u64,
    name: String,
    material: usize,
) -> anyhow::Result<Mesh> {
    let (document, buffers, _) = gltf::import(path)?;
    let mut mesh = Mesh {
        entity,
        name,
        vertices: Vec::new(),
        indices: Vec::new(),
        material,
    };
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next())
        .ok_or_else(|| anyhow::anyhow!("glTF has no scene"))?;
    let mut stack: Vec<_> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let matrix = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        stack.extend(node.children().map(|n| (n, matrix)));
        let Some(source) = node.mesh() else {
            continue;
        };
        for primitive in source.primitives() {
            anyhow::ensure!(
                primitive.mode() == gltf::mesh::Mode::Triangles,
                "glTF primitive must be triangles"
            );
            let read =
                primitive.reader(|buffer| buffers.get(buffer.index()).map(|b| b.0.as_slice()));
            let positions: Vec<_> = read
                .read_positions()
                .ok_or_else(|| anyhow::anyhow!("mesh has no positions"))?
                .collect();
            let normals: Vec<_> = read
                .read_normals()
                .map(Iterator::collect)
                .unwrap_or_default();
            let texcoords: Vec<_> = read
                .read_tex_coords(0)
                .map(|u| u.into_f32().collect())
                .unwrap_or_default();
            let base = u32::try_from(mesh.vertices.len())?;
            let normal_matrix = matrix.inverse().transpose();
            for (i, position) in positions.into_iter().enumerate() {
                let normal = normal_matrix
                    .transform_vector3(normals.get(i).copied().map(Vec3::from).unwrap_or(Vec3::Y))
                    .normalize_or_zero();
                mesh.vertices.push(Vertex {
                    position: matrix.transform_point3(position.into()).into(),
                    normal: normal.into(),
                    uv: texcoords.get(i).copied().unwrap_or([0.; 2]),
                    tangent: tangent(normal),
                });
            }
            if let Some(indices) = read.read_indices() {
                mesh.indices.extend(indices.into_u32().map(|i| base + i));
            } else {
                mesh.indices.extend(base..mesh.vertices.len() as u32);
            }
        }
    }
    anyhow::ensure!(
        !mesh.indices.is_empty(),
        "glTF contains no triangle geometry"
    );
    Ok(mesh)
}

/// A unit tangent perpendicular to `normal`, for loaders without UV tangents. A fixed
/// +X tangent is parallel to ±X normals, and the fidelity shading basis
/// (`normalize(t - n·dot(t, n))`) then turns those faces black.
pub(crate) fn tangent(normal: Vec3) -> [f32; 4] {
    normal.any_orthonormal_vector().extend(1.).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tangent_is_perpendicular_to_axis_normals() {
        for normal in [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ] {
            let t = Vec3::from_slice(&tangent(normal)[..3]);
            assert!((t.length() - 1.).abs() < 1e-5, "{normal}");
            assert!(t.dot(normal).abs() < 1e-5, "{normal}");
        }
    }
}
