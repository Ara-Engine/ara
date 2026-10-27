use super::*;

fn triangle() -> Scene {
    Scene {
        geometry: vec![Geometry {
            key: [7; 32],
            vertices: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]
                .map(|position| Vertex {
                    position,
                    normal: [0.0, 1.0, 0.0],
                    uv: [0.0; 2],
                    tangent: [1.0, 0.0, 0.0, -1.0],
                    uv1: [0.0; 2],
                    terrain_weights: 0,
                    reserved: 0,
                })
                .to_vec(),
            lods: vec![Lod {
                indices: vec![0, 2, 1],
                error: 0.0,
            }],
            bounds: [Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0)],
            deforming: false,
            revision: 0,
        }],
        instances: vec![Instance {
            entity: 1,
            name: "triangle".into(),
            geometry: 0,
            material: 0,
            world: Mat4::IDENTITY,
            previous_world: Mat4::IDENTITY,
        }],
        materials: vec![Material::default()],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ONE,
        environment: vec![[1.0; 4]],
        environment_size: [1, 1],
    }
}

#[test]
fn independent_uv1_transform_has_the_native_shader_layout() {
    assert_eq!(std::mem::size_of::<Vertex>(), 64);
    assert_eq!(std::mem::size_of::<TextureTransform>(), 32);
    assert_eq!(std::mem::size_of::<MaterialExtras>(), 240);
    assert_eq!(std::mem::size_of::<Material>(), 320);
    let mut material = Material::default();
    material.extras.uv[TextureSlot::Normal as usize] = TextureTransform {
        offset: [1.0, 2.0],
        scale: [2.0, 3.0],
        rotation: std::f32::consts::FRAC_PI_2,
        tex_coord: 1,
        ..TextureTransform::default()
    };
    let transformed =
        material.extras.uv[TextureSlot::Normal as usize].apply([0.25, 0.5], [0.1, 0.2]);
    assert!((transformed[0] - 0.4).abs() < 1e-6 && (transformed[1] - 2.2).abs() < 1e-6);
    assert_eq!(
        material.extras.uv[TextureSlot::BaseColor as usize].apply([0.25, 0.5], [0.1, 0.2]),
        [0.25, 0.5]
    );
}

#[test]
fn more_than_4096_instances_share_geometry_and_keep_mirrored_bounds() {
    let mut scene = triangle();
    let matrix = Mat4::from_scale_rotation_translation(
        Vec3::new(-2.0, 3.0, 4.0),
        glam::Quat::IDENTITY,
        Vec3::new(10.0, 20.0, 30.0),
    );
    scene.instances[0].world = matrix;
    assert_eq!(
        scene.instances[0].bounds(&scene.geometry[0]),
        [Vec3::new(8.0, 20.0, 30.0), Vec3::new(10.0, 26.0, 42.0)]
    );
    for entity in 2..=5000 {
        scene.instances.push(Instance {
            entity,
            name: String::new(),
            geometry: 0,
            material: 0,
            world: matrix,
            previous_world: matrix,
        });
    }
    scene.validate().unwrap();
    assert_eq!(scene.geometry.len(), 1);
    assert_eq!(scene.geometry[0].vertices.len(), 3);
    scene.instances[4999].entity = 1;
    assert!(
        scene.validate().is_err(),
        "stable instance IDs must not alias"
    );
}

#[test]
fn bad_material_references_and_nested_splats_are_rejected_before_upload() {
    let mut scene = triangle();
    scene.materials[0].pbr.textures[0] = 0;
    assert!(
        scene.validate().is_err(),
        "no resident texture was registered"
    );
    scene.materials[0].pbr.textures[0] = NO_TEXTURE;
    scene.materials[0].extras.terrain_layers = 1;
    scene.materials[0].extras.layer_materials[0] = 0;
    assert!(scene.validate().is_err(), "terrain layers cannot recurse");
    scene.materials.push(Material::default());
    scene.materials[0].extras.layer_materials[0] = 1;
    scene.validate().unwrap();
}

#[test]
fn volume_packs_ior_thickness_and_absorption_for_the_refraction_shader() {
    assert_eq!(Volume::default().pack(), [0; 2]);
    assert_eq!(Volume::unpack([0; 2]), (1.5, 0., [0.; 3]));
    let volume = Volume {
        ior: 1.33,
        thickness: 0.6,
        attenuation_color: [0.2, 0.55, 0.45],
        attenuation_distance: 1.,
    };
    let (ior, thickness, absorption) = Volume::unpack(volume.pack());
    assert!((ior - 1.33).abs() < 1e-4);
    assert!((thickness - 0.6).abs() < 1e-3);
    for (got, colour) in absorption.into_iter().zip(volume.attenuation_color) {
        let expected = -colour.ln();
        assert!((got - expected).abs() < expected * 0.01, "{got} vs {expected}");
    }
    let mut scene = triangle();
    scene.materials[0].extras.volume = volume.pack();
    assert!(scene.validate().is_ok());
    scene.materials[0].extras.volume[0] |= 0x7c00;
    assert!(scene.validate().is_err());
}
