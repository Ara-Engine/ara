use super::*;
use ara_runtime::{
    extract::{Draw, LightView},
    model::{ModelLod, ModelMaterial},
};
use glam::Mat4;
use std::sync::Arc;

fn draw(geometry: Geometry) -> Draw {
    Draw {
        entity: 17,
        address: "e_model".into(),
        label: "Model (e_model)".into(),
        geometry,
        world: Mat4::from_translation(Vec3::new(3.0, 2.0, 1.0)),
        color: [0.5, 0.75, 1.0, 0.5],
        roughness: 0.5,
        metallic: 0.0,
        emissive: [2.0, 3.0, 4.0],
        skin: None,
    }
}

#[test]
fn model_factors_alpha_emission_and_cooked_lod_reach_fidelity() {
    let source = crate::geometry::shape(ara_runtime::components::MeshShape::Box, [1.0; 3]);
    let model = Model {
        asset: "as_model".into(),
        digest: ara_asset::Digest([7; 32]),
        primitives: vec![Primitive {
            vertices: source.vertices,
            indices: source.indices,
            lods: vec![ModelLod {
                indices: vec![0, 1, 2],
                error: 0.001,
            }],
            material: Some(0),
            skin: None,
        }],
        materials: vec![ModelMaterial {
            base_color: [0.8, 0.4, 0.2, 0.6],
            metallic: 0.7,
            roughness: 0.3,
            emissive: [5.0, 6.0, 7.0],
            base_color_texture: None,
            normal_texture: None,
            normal_scale: 0.8,
            metallic_roughness_texture: None,
            emissive_texture: None,
            alpha_mode: 1,
            alpha_cutoff: 0.4,
        }],
        textures: Vec::new(),
        skins: Vec::new(),
        bounds: [[-0.5; 3], [0.5; 3]],
    };
    let mut frame = RenderFrame::default();
    frame.draws.push(draw(Geometry::Model {
        model: Arc::new(model),
        primitive: 0,
    }));
    let (scene, stats) = scene(&frame, Content::default()).expect("valid scene");
    let m = &scene.materials[0];
    assert_eq!(m.base_color, [0.4, 0.3, 0.2, 0.3]);
    assert_eq!(m.emissive_roughness, [10.0, 18.0, 28.0, 0.3]);
    assert_eq!(m.surface, [0.7, 0.8, 1.0, 0.4]);
    assert_eq!(m.flags[0], 1, "cutout stays cutout");
    assert_eq!(scene.meshes[0].vertices[0].position, [3.5, 1.5, 1.5]);
    assert_eq!(stats.triangles, 1);
    assert_eq!(stats.finest_triangles, 12);
    assert_eq!(stats.emissive_materials, 1);
}

#[test]
fn all_local_lights_and_environment_survive_the_adapter() {
    let mut frame = RenderFrame::default();
    frame.draws.push(draw(Geometry::Shape {
        shape: ara_runtime::components::MeshShape::Box,
        size: [1.0; 3],
    }));
    frame.lights = (0..100)
        .map(|i| LightView {
            kind: LightKind::Spot,
            position: [i as f32, 2.0, 0.0],
            direction: [0.0, -1.0, 0.0],
            color: [1.0, 0.5, 0.2],
            lumens: 500.0,
            range: 10.0,
            cone: [15.0, 30.0],
        })
        .collect();
    let (scene, stats) = scene(
        &frame,
        Content {
            environment: Some(EnvironmentMap {
                size: [1, 1],
                pixels: vec![[3.0, 2.0, 1.0, 1.0]],
            }),
            ..Default::default()
        },
    )
    .expect("valid scene");
    assert_eq!(stats.lights, 100);
    assert_eq!(scene.lights[99].position_range, [99.0, 2.0, 0.0, 10.0]);
    assert_eq!(scene.lights[0].parameters[0], 1.0);
    assert_eq!(scene.environment, [[3.0, 2.0, 1.0, 1.0]]);
    assert_eq!(
        scene.materials[0].flags[0], 2,
        "shape opacity remains blended"
    );
}

#[test]
fn every_pbr_map_is_resolved_once_per_model_generation() {
    use ara_asset_format::{TextureFormat, write_ktx2};
    use ara_runtime::model::{ModelTexture, TextureUsage};
    let mut model = Model {
        asset: "as_textured".into(),
        digest: ara_asset::Digest([3; 32]),
        primitives: Vec::new(),
        materials: Vec::new(),
        textures: Vec::new(),
        skins: Vec::new(),
        bounds: [[0.0; 3], [1.0; 3]],
    };
    for (name, format, usage) in [
        ("base", TextureFormat::Bc7Srgb, TextureUsage::Color),
        ("normal", TextureFormat::Bc5, TextureUsage::Normal),
        ("orm", TextureFormat::Bc7, TextureUsage::Linear),
        ("emissive", TextureFormat::Bc7Srgb, TextureUsage::Color),
    ] {
        model.textures.push(ModelTexture {
            name: name.into(),
            usage,
            ktx2: write_ktx2(4, 4, format, &[vec![64; 16]])
                .expect("valid compressed level")
                .into(),
        });
    }
    let source = ModelMaterial {
        base_color: [1.0; 4],
        metallic: 0.9,
        roughness: 0.2,
        emissive: [1.0; 3],
        base_color_texture: Some(0),
        normal_texture: Some(1),
        normal_scale: 0.5,
        metallic_roughness_texture: Some(2),
        emissive_texture: Some(3),
        alpha_mode: 2,
        alpha_cutoff: 0.5,
    };
    let mut builder = Builder::default();
    for _ in 0..2 {
        let mut material = Material::default();
        builder
            .model_material(&mut material, &model, &source, [1.0; 4], [2.0; 3])
            .expect("all cooked maps resolve");
        assert_eq!(material.textures, [2, 3, 4, 5]);
        assert_eq!(material.flags[0], 2);
        assert_eq!(material.emissive_roughness[..3], [2.0; 3]);
    }
    assert_eq!(
        builder.textures.len(),
        6,
        "four shared maps plus two defaults"
    );
    assert!(builder.textures[2].srgb && builder.textures[5].srgb);
    assert!(!builder.textures[3].srgb && !builder.textures[4].srgb);
}
