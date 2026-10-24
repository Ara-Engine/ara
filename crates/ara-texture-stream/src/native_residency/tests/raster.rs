use super::*;
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    instanced,
    scene::CameraSpec,
};
use glam::{Mat4, Vec3};

#[test]
fn native_raster_samples_compressed_raw_maps_with_independent_uvs_and_decode_policy() -> Result<()>
{
    assert_eq!(
        include_str!("../../../../../shaders/native_sample.slangh"),
        crate::native_shader_source()
    );
    fn block(value: u8) -> [u8; 16] {
        let mut bits = 1u128 << 6;
        let mut offset = 7;
        for channel in [value, value, value, 254] {
            for _ in 0..2 {
                bits |= u128::from(channel / 2) << offset;
                offset += 7;
            }
        }
        bits.to_le_bytes()
    }
    let source = NativeSource::from_cooked(
        write_ktx2(
            8,
            4,
            TextureFormat::Bc7Srgb,
            &[[block(128), block(224)].concat()],
        )?
        .into(),
    )?;
    let mut textures = NativeResidency::new(NativeConfig {
        resident_bytes: 1024,
        upload_bytes: 1024,
    })?;
    textures.register_view([1; 32], Arc::new(source), NativeView::Raw)?;
    let mut left = instanced::Material::default();
    left.pbr.textures[0] = 0;
    left.pbr.flags[1] = ara_render::scene::BASE_COLOR_DECODE_SRGB;
    left.extras.uv[0].tex_coord = 1;
    let mut right = left;
    right.extras.uv[0].tex_coord = 0;
    let scene = instanced::Scene {
        geometry: vec![instanced::Geometry {
            key: [1; 32],
            vertices: [
                [-0.4, -0.4, 0.],
                [0.4, -0.4, 0.],
                [0.4, 0.4, 0.],
                [-0.4, 0.4, 0.],
            ]
            .into_iter()
            .map(|position| instanced::Vertex {
                position,
                normal: [0., 0., 1.],
                tangent: [1., 0., 0., 1.],
                uv: [0.75, 0.5],
                uv1: [0.25, 0.5],
                terrain_weights: 0,
                reserved: 0,
            })
            .collect(),
            lods: vec![instanced::Lod {
                indices: vec![0, 1, 2, 0, 2, 3],
                error: 0.,
            }],
            bounds: [Vec3::new(-0.4, -0.4, 0.), Vec3::new(0.4, 0.4, 0.)],
            deforming: false,
            revision: 0,
        }],
        instances: [-0.6, 0.6]
            .into_iter()
            .enumerate()
            .map(|(id, x)| instanced::Instance {
                entity: id as u64 + 1,
                name: format!("uv{id}"),
                geometry: 0,
                material: id,
                world: Mat4::from_translation(Vec3::new(x, 0., 0.)),
                previous_world: Mat4::IDENTITY,
            })
            .collect(),
        materials: vec![left, right],
        textures: 1,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ZERO,
        environment: vec![[0., 0., 0., 1.]],
        environment_size: [1, 1],
    };
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-texture-stream/native-raster")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(textures),
        [192, 108],
        &[],
        root.join("cache"),
    )?;
    let capture = renderer.capture(&CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 0., 3.),
            target: Vec3::ZERO,
            vertical_fov: 0.8,
        },
        size: [192, 108],
        mode: ViewMode::Albedo,
        deterministic: true,
        exposure: 1.,
        settle_frames: 1,
        tick: 0,
        seed: 7,
        marks: None,
    })?;
    capture.write(&root.join("independent-uv-raw-bc7.png"))?;
    let left = capture.image.get_pixel(70, 54).0;
    let right = capture.image.get_pixel(122, 54).0;
    assert!(left[0].abs_diff(128) <= 2, "UV1 sample: {left:?}");
    assert!(right[0].abs_diff(224) <= 2, "UV0 sample: {right:?}");
    assert!(left[..3].windows(2).all(|v| v[0] == v[1]));
    let stats = renderer.native_texture_stats().unwrap();
    assert_eq!(
        stats.resident_bytes, 32,
        "two native BC7 blocks, with no RGBA/mip atlas allocation"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
