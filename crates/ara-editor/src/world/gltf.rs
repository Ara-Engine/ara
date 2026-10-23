//! Temporary example-style glTF geometry loader; ara-asset will own cooked assets.
use ara_render::scene::{Mesh, Vertex};
use glam::{Mat4, Vec3};
use std::path::Path;

pub(super) fn load(
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
                mesh.vertices.push(Vertex {
                    position: matrix.transform_point3(position.into()).into(),
                    normal: normal_matrix
                        .transform_vector3(
                            normals.get(i).copied().map(Vec3::from).unwrap_or(Vec3::Y),
                        )
                        .normalize_or_zero()
                        .into(),
                    uv: texcoords.get(i).copied().unwrap_or([0.; 2]),
                    tangent: [1., 0., 0., 1.],
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
