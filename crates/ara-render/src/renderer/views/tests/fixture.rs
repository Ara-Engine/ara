use super::*;

pub(in crate::renderer) fn scene() -> instanced::Scene {
    instanced::Scene {
        geometry: vec![instanced::Geometry {
            key: [91; 32],
            vertices: [[-0.7, -0.6, 0.], [0.7, -0.6, 0.], [0., 0.7, 0.]]
                .into_iter()
                .map(|position| instanced::Vertex {
                    position,
                    normal: [0., 0., 1.],
                    uv: [0.; 2],
                    tangent: [1., 0., 0., 1.],
                    uv1: [0.; 2],
                    terrain_weights: 0,
                    reserved: 0,
                })
                .collect(),
            lods: vec![instanced::Lod {
                indices: vec![0, 1, 2],
                error: 0.,
            }],
            bounds: [Vec3::new(-0.7, -0.6, 0.), Vec3::new(0.7, 0.7, 0.)],
            deforming: true,
            revision: 0,
        }],
        instances: vec![instanced::Instance {
            entity: 7,
            name: "shared triangle".into(),
            geometry: 0,
            material: 0,
            world: Mat4::IDENTITY,
            previous_world: Mat4::IDENTITY,
        }],
        materials: vec![instanced::Material {
            pbr: crate::scene::Material {
                base_color: [0.9, 0.08, 0.02, 1.],
                textures: [u32::MAX; 4],
                surface: [0., 1., 0., 0.5],
                ..Default::default()
            },
            ..Default::default()
        }],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Z,
        sun_lux: Vec3::splat(10.),
        environment: vec![[0.04, 0.04, 0.04, 1.]],
        environment_size: [1, 1],
    }
}
