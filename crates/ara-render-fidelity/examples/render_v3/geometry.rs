//! Shared native geometry: 4,000 bodies retain one model-space allocation.
use ara_frame::instanced::{Geometry, Instance, Lod, Material, Scene, Vertex};
use glam::{Mat4, Vec3};

pub fn cube(triangles: u32) -> Geometry {
    let grids = match triangles {
        108 => [(3, 3); 6],
        1000 => [(9, 9), (9, 9), (9, 9), (9, 9), (8, 11), (8, 11)],
        _ => [(1, 1); 6],
    };
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (face, (nx, ny)) in grids.into_iter().enumerate() {
        let (normal, u, v) = match face {
            0 => (Vec3::X, -Vec3::Z, Vec3::Y),
            1 => (-Vec3::X, Vec3::Z, Vec3::Y),
            2 => (Vec3::Y, Vec3::X, -Vec3::Z),
            3 => (-Vec3::Y, Vec3::X, Vec3::Z),
            4 => (Vec3::Z, Vec3::X, Vec3::Y),
            _ => (-Vec3::Z, -Vec3::X, Vec3::Y),
        };
        let start = vertices.len() as u32;
        for y in 0..=ny {
            for x in 0..=nx {
                let uv = [x as f32 / nx as f32, y as f32 / ny as f32];
                vertices.push(Vertex {
                    position: (normal * 0.5 + u * (uv[0] - 0.5) + v * (uv[1] - 0.5)).into(),
                    normal: normal.into(),
                    tangent: [u.x, u.y, u.z, 1.],
                    uv,
                    uv1: uv,
                    terrain_weights: 0,
                    color: u32::MAX,
                });
            }
        }
        for y in 0..ny {
            for x in 0..nx {
                let a = start + y * (nx + 1) + x;
                let b = a + 1;
                let c = a + nx + 1;
                let d = c + 1;
                indices.extend([a, b, d, a, d, c]);
            }
        }
    }
    Geometry {
        key: {
            let mut key = [0; 32];
            key[..4].copy_from_slice(&triangles.to_le_bytes());
            key[4] = 3;
            key
        },
        vertices,
        lods: vec![Lod { indices, error: 0. }],
        bounds: [Vec3::splat(-0.5), Vec3::splat(0.5)],
        deforming: false,
        vertex_colors: false,
        revision: 0,
    }
}

pub fn material(color: [f32; 3], roughness: f32) -> Material {
    let mut m = Material::default();
    m.pbr.textures = [u32::MAX; 4];
    m.pbr.base_color = [color[0], color[1], color[2], 1.];
    m.pbr.emissive_roughness[3] = roughness;
    m
}

pub fn add(
    scene: &mut Scene,
    id: u64,
    geometry: usize,
    material: usize,
    position: Vec3,
    scale: Vec3,
) {
    let world = Mat4::from_scale_rotation_translation(scale, glam::Quat::IDENTITY, position);
    scene.instances.push(Instance {
        entity: id,
        name: format!("fixture-{id}"),
        geometry,
        material,
        world,
        previous_world: world,
        casts_shadows: true,
        paint: None,
        receives_decals: true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_triangle_counts_and_winding_are_real() {
        for n in [12, 108, 1000] {
            let g = cube(n);
            assert_eq!(g.lods[0].indices.len(), n as usize * 3);
            for t in g.lods[0].indices.chunks_exact(3) {
                let a = Vec3::from(g.vertices[t[0] as usize].position);
                let b = Vec3::from(g.vertices[t[1] as usize].position);
                let c = Vec3::from(g.vertices[t[2] as usize].position);
                assert!(
                    (b - a)
                        .cross(c - a)
                        .dot(Vec3::from(g.vertices[t[0] as usize].normal))
                        > 0.
                );
            }
        }
    }
}
