//! Offline OBJ material conversion and UV-derived tangent frames.
use anyhow::Result;
use ara_render::scene::{Mesh, Scene, Texture};
use glam::{Vec2, Vec3};
use std::path::Path;

/// Convert authored scalar bump maps and specular/gloss maps, without inventing
/// clearcoat or fabric normals absent from the source asset.
pub(crate) fn detail(scene: &mut Scene, path: &Path) -> Result<()> {
    let mut index = None;
    for line in std::fs::read_to_string(path.join("sponza.mtl"))?.lines() {
        let words = line.split_whitespace().collect::<Vec<_>>();
        if words.first() == Some(&"newmtl") {
            index = Some(index.map_or(0, |i| i + 1));
        } else if let Some(i) = index {
            let Some(name) = words.last() else { continue };
            let kind = words.first().copied().unwrap_or("");
            if kind != "map_bump" && kind != "bump" && kind != "map_Ks" {
                continue;
            }
            let source = image::open(path.join(name.replace('\\', "/")))?.to_rgba8();
            let size = [source.width(), source.height()];
            let rgba = if kind == "map_Ks" {
                // MTL map_Ks is specular reflectance, not a roughness map.
                // Preserve Ns roughness; pack bounded dielectric intensity in A.
                source
                    .pixels()
                    .flat_map(|p| [255, 255, 255, p[0]])
                    .collect()
            } else {
                // The archive's bump maps are heights, not tangent-space RGB normals.
                let height = |x: u32, y: u32| {
                    f32::from(source.get_pixel(x % size[0], y % size[1])[0]) / 255.
                };
                let mut out = Vec::with_capacity(source.len());
                for y in 0..size[1] {
                    for x in 0..size[0] {
                        let dx = height(x + 1, y) - height(x + size[0] - 1, y);
                        let dy = height(x, y + 1) - height(x, y + size[1] - 1);
                        let n = Vec3::new(-dx * 2., -dy * 2., 1.).normalize();
                        out.extend(n.to_array().map(|v| ((v * 0.5 + 0.5) * 255.).round() as u8));
                        out.push(255);
                    }
                }
                out
            };
            let texture = scene.textures.len() as u32;
            scene.textures.push(Texture {
                size,
                rgba,
                srgb: false,
            });
            scene.materials[i].textures[if kind == "map_Ks" { 2 } else { 1 }] = texture;
            if kind == "map_Ks" {
                ara_render_fidelity::material::MaterialDetail::default()
                    .apply(&mut scene.materials[i])?;
            }
        }
    }
    for mesh in &mut scene.meshes {
        tangents(mesh);
    }
    Ok(())
}

/// Accumulate UV derivatives, then orthogonalize and retain mirrored-UV handedness.
fn tangents(mesh: &mut Mesh) {
    let mut tangent = vec![Vec3::ZERO; mesh.vertices.len()];
    let mut bitangent = vec![Vec3::ZERO; mesh.vertices.len()];
    for tri in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| mesh.vertices[i as usize]);
        let e1 = Vec3::from_array(b.position) - Vec3::from_array(a.position);
        let e2 = Vec3::from_array(c.position) - Vec3::from_array(a.position);
        let u1 = Vec2::from_array(b.uv) - Vec2::from_array(a.uv);
        let u2 = Vec2::from_array(c.uv) - Vec2::from_array(a.uv);
        let determinant = u1.perp_dot(u2);
        if determinant.abs() < 1e-10 {
            continue;
        }
        let t = (e1 * u2.y - e2 * u1.y) / determinant;
        let b = (e2 * u1.x - e1 * u2.x) / determinant;
        for i in tri {
            tangent[*i as usize] += t;
            bitangent[*i as usize] += b;
        }
    }
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let n = Vec3::from_array(v.normal);
        let t = (tangent[i] - n * n.dot(tangent[i]))
            .try_normalize()
            .unwrap_or_else(|| {
                n.cross(if n.y.abs() < 0.99 { Vec3::Y } else { Vec3::X })
                    .normalize()
            });
        let sign = if n.cross(t).dot(bitangent[i]) < 0. {
            -1.
        } else {
            1.
        };
        v.tangent = t.extend(sign).to_array();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uv_tangent_tracks_rotation_and_mirroring() {
        let mut mesh = crate::scenes::quad(1, "UV test", Vec3::ZERO, Vec3::X, Vec3::Z, 0);
        tangents(&mut mesh);
        let first = mesh.vertices[0].tangent;
        for v in &mut mesh.vertices {
            v.uv[0] = -v.uv[0];
        }
        tangents(&mut mesh);
        assert!(Vec3::from_slice(&first).dot(Vec3::from_slice(&mesh.vertices[0].tangent)) < -0.99);
        assert_eq!(first[3], -mesh.vertices[0].tangent[3]);
    }
}
