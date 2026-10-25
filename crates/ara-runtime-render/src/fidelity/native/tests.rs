#![allow(clippy::unwrap_used)]
use super::*;
use ara_asset::Digest as AssetDigest;
use ara_asset_format::{TextureFormat, write_ktx2};
use ara_runtime::{
    components::MeshShape,
    extract::Geometry,
    material::TextureGeneration,
    model::{ModelMaterial, ModelTexture, TextureTransform, TextureUsage},
};

mod bounds;
mod document;
mod generations;
mod geometry;
mod publication;
mod sync;
mod terrain;
mod terrain_resolution;

fn draw(entity: u64) -> Draw {
    Draw {
        entity,
        address: format!("e_{entity}"),
        label: "Shared box".into(),
        geometry: Geometry::Shape {
            shape: MeshShape::Box,
            size: [1.0; 3],
        },
        world: glam::Mat4::from_translation(glam::Vec3::new(entity as f32, 0.0, 0.0)),
        color: [1.0; 4],
        roughness: 0.7,
        metallic: 0.0,
        emissive: [2.0, 3.0, 4.0],
        emissive_factor: Some([0.5, 1.0, 2.0]),
        material: None,
        skin: None,
    }
}

fn config() -> NativeConfig {
    NativeConfig {
        resident_bytes: 1 << 20,
        upload_bytes: 1 << 20,
    }
}

#[test]
fn thousands_of_instances_share_local_geometry_without_a_mesh_cap() {
    let mut frame = RenderFrame::default();
    frame.draws = (0..5000).map(draw).collect();
    let prepared = prepare(&frame, Content::default(), config()).unwrap();
    assert_eq!(prepared.scene.geometry.len(), 1);
    assert_eq!(prepared.scene.geometry[0].vertices.len(), 24);
    assert_eq!(prepared.scene.instances.len(), 5000);
    assert_eq!(prepared.scene.materials.len(), 1);
    assert_eq!(prepared.scene.textures, 0);
    assert_eq!(
        prepared.scene.geometry[0].vertices[0].position,
        [0.5, -0.5, 0.5]
    );
    assert_eq!(prepared.scene.instances[4999].world.w_axis.x, 4999.0);
}

#[test]
fn native_material_retains_each_uv_channel_and_never_folds_maps_into_factors() {
    let texture = ModelTexture {
        name: "shared.ktx2".into(),
        usage: TextureUsage::Color,
        ktx2: write_ktx2(4, 4, TextureFormat::Bc7Srgb, &[vec![0; 16]])
            .unwrap()
            .into(),
    };
    let mut material = ModelMaterial {
        base_color: [0.25, 0.5, 0.75, 0.8],
        roughness: 0.3,
        metallic: 0.8,
        base_color_texture: Some(0),
        normal_texture: Some(0),
        metallic_roughness_texture: Some(0),
        emissive_texture: Some(0),
        occlusion_texture: Some(0),
        transmission_texture: Some(0),
        emissive: [100.0; 3],
        occlusion_strength: 0.25,
        transmission_factor: 0.6,
        ..Default::default()
    };
    let transforms = std::array::from_fn::<_, 6, _>(|i| TextureTransform {
        offset: [i as f32, -1.0],
        scale: [2.0, 3.0],
        rotation: i as f32 * 0.1,
        tex_coord: (i % 2) as u32,
    });
    [
        material.base_color_uv,
        material.normal_uv,
        material.metallic_roughness_uv,
        material.emissive_uv,
        material.occlusion_uv,
        material.transmission_uv,
    ] = transforms;
    let asset = Arc::new(MaterialAsset {
        asset: "gr_surface".into(),
        digest: AssetDigest::of(b"graph"),
        material,
        textures: vec![texture],
        texture_generations: vec![TextureGeneration {
            asset: "as_color".into(),
            digest: AssetDigest::of(b"image"),
        }],
        base_color_decode_srgb: true,
        emissive_decode_srgb: true,
    });
    let mut frame = RenderFrame::default();
    let mut instance = draw(1);
    instance.color = [0.5; 4];
    instance.material = Some(asset);
    frame.draws.push(instance);
    let prepared = prepare(&frame, Content::default(), config()).unwrap();
    let out = &prepared.scene.materials[0];
    assert_eq!(
        prepared.scene.textures, 1,
        "all six slots share one raw view"
    );
    assert_eq!(out.pbr.base_color, [0.125, 0.25, 0.375, 0.4]);
    assert_eq!(out.pbr.emissive_roughness, [1.0, 3.0, 8.0, 0.3]);
    assert_eq!(out.extras.occlusion_strength, 0.25);
    assert_eq!(out.extras.transmission_factor, 0.6);
    assert_eq!(out.extras.reconstruct_normal_basis, 1);
    for (source, actual) in transforms.into_iter().zip(out.extras.uv) {
        assert_eq!(actual.offset, source.offset);
        assert_eq!(actual.scale, source.scale);
        assert_eq!(actual.rotation, source.rotation);
        assert_eq!(actual.tex_coord, source.tex_coord);
    }
    assert_eq!(
        out.pbr.flags[1],
        ara_render::scene::BASE_COLOR_DECODE_SRGB | ara_render::scene::EMISSIVE_DECODE_SRGB
    );
}

#[test]
fn hundreds_of_native_maps_remain_bound_instead_of_becoming_constant_factors() {
    let mut frame = RenderFrame::default();
    for index in 0u32..300 {
        let mut block = vec![0; 8];
        block[0] = index as u8;
        block[1] = (index >> 8) as u8;
        let texture = ModelTexture {
            name: format!("{index}.ktx2"),
            usage: TextureUsage::Linear,
            ktx2: write_ktx2(4, 4, TextureFormat::Bc4, &[block])
                .unwrap()
                .into(),
        };
        let mut instance = draw(u64::from(index));
        instance.material = Some(Arc::new(MaterialAsset {
            asset: format!("gr_surface{index}"),
            digest: AssetDigest::of(&index.to_le_bytes()),
            material: ModelMaterial {
                emissive_texture: Some(0),
                ..Default::default()
            },
            textures: vec![texture],
            texture_generations: vec![TextureGeneration {
                asset: format!("as_image{index}"),
                digest: AssetDigest::of(&index.to_le_bytes()),
            }],
            base_color_decode_srgb: false,
            emissive_decode_srgb: false,
        }));
        frame.draws.push(instance);
    }
    let prepared = prepare(&frame, Content::default(), config()).unwrap();
    assert_eq!(prepared.scene.geometry.len(), 1);
    assert_eq!(prepared.scene.textures, 300);
    assert_eq!(prepared.textures.content_keys().count(), 300);
    for (index, material) in prepared.scene.materials.iter().enumerate() {
        assert_eq!(material.pbr.textures[3], index as u32);
        assert_eq!(material.pbr.emissive_roughness[..3], [1.0, 3.0, 8.0]);
    }
}
