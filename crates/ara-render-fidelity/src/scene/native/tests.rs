use super::*;
mod motion;
mod skin;

#[test]
fn native_instance_rows_preserve_affine_matrix_and_normal_offsets() {
    assert_eq!(size_of::<packing::InstanceRow>(), 192);
    assert_eq!(std::mem::offset_of!(packing::InstanceRow, world), 16);
    assert_eq!(std::mem::offset_of!(packing::InstanceRow, previous), 80);
    assert_eq!(std::mem::offset_of!(packing::InstanceRow, normal), 144);
}

#[test]
fn native_sampling_include_tracks_the_residency_contract() {
    assert_eq!(
        include_str!("../../../shaders/native_sample.slangh"),
        ara_texture_stream::native_shader_source()
    );
}

#[test]
fn native_fidelity_traces_shared_transforms_and_independent_emissive_uv1() -> Result<()> {
    use crate::{FidelityView, FrameInput, Mode, PostSettings, Settings, Tier, Upscaler};
    use ara_render::instanced::{Geometry, Instance, Lod, Material, Vertex};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("../../target/renderer-unification/native-instancing");
    let mut gpu = VulkanDevice::new(&[], output.join("cache"))?;
    if !gpu.context().info.ray_tracing {
        return Ok(());
    }
    let vertices = [
        [-0.4, -0.4, 0.0],
        [0.4, -0.4, 0.0],
        [0.4, 0.4, 0.0],
        [-0.4, 0.4, 0.0],
    ]
    .map(|position| Vertex {
        position,
        normal: [0.0, 0.0, 1.0],
        tangent: [1.0, 0.0, 0.0, 1.0],
        uv: [0.75, 0.5],
        uv1: [0.25, 0.5],
        terrain_weights: 0,
        reserved: 0,
    })
    .to_vec();
    let mut left = Material::default();
    left.pbr.emissive_roughness = [3.0, 0.0, 0.0, 1.0];
    left.pbr.textures[3] = 0;
    left.pbr.flags[1] = ara_render::scene::EMISSIVE_DECODE_SRGB;
    left.extras.uv[3].tex_coord = 1;
    let mut right = left;
    right.pbr.emissive_roughness = [0.0, 3.0, 0.0, 1.0];
    right.extras.uv[3].tex_coord = 0;
    let mut scene = NativeScene {
        geometry: vec![Geometry {
            key: [1; 32],
            vertices,
            lods: vec![Lod {
                indices: vec![0, 1, 2, 0, 2, 3],
                error: 0.0,
            }],
            bounds: [Vec3::new(-0.4, -0.4, 0.0), Vec3::new(0.4, 0.4, 0.0)],
            deforming: true,
            revision: 0,
        }],
        instances: [-0.6, 0.6]
            .into_iter()
            .enumerate()
            .map(|(id, x)| {
                let world = glam::Mat4::from_scale_rotation_translation(
                    Vec3::new(if id == 0 { -1.0 } else { 1.0 }, 1.0, 1.0),
                    glam::Quat::IDENTITY,
                    Vec3::new(x, 0.0, 0.0),
                );
                Instance {
                    entity: id as u64,
                    name: format!("instance{id}"),
                    geometry: 0,
                    material: id,
                    world,
                    previous_world: world,
                }
            })
            .collect(),
        materials: vec![left, right],
        textures: 1,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ZERO,
        environment: vec![[0.0, 0.0, 0.0, 1.0]],
        environment_size: [1, 1],
    };
    let mut textures = NativeResidency::new(ara_texture_stream::NativeConfig {
        resident_bytes: 1024,
        upload_bytes: 1024,
    })?;
    let source = ara_texture_stream::NativeSource::from_cooked(native_color_ktx().into())?;
    textures.register_view(
        [1; 32],
        std::sync::Arc::new(source),
        ara_texture_stream::NativeView::Raw,
    )?;
    let size = [128, 64];
    let settings = Settings {
        render_size: size,
        display_size: size,
        tier: Tier::Rt,
        mode: Mode::Reference {
            samples: 8,
            bounces: 1,
        },
        upscaler: Upscaler::Taau,
        probe_ray_budget: 64,
        reflection_cutoff: 1.0,
        post: PostSettings {
            bloom: 0.0,
            fog_density: 0.0,
            aperture: 0.0,
            shutter: 0.0,
            ev100: Some(0.0),
            ..Default::default()
        },
    };
    let camera = ara_render::scene::CameraSpec::LookAt {
        position: Vec3::new(0.0, 0.0, 3.0),
        target: Vec3::ZERO,
        vertical_fov: 60.0f32.to_radians(),
    };
    let mut view = FidelityView::new_native(&mut gpu, &scene, textures, settings, None)?;
    view.render(
        &mut gpu,
        &FrameInput {
            camera: &camera,
            tick: 0,
            delta_seconds: 1.0 / 60.0,
            reset: true,
            lighting_changed: false,
            deterministic: true,
        },
    )?;
    let radiance = view.capture_radiance(&mut gpu)?;
    assert!(
        !view
            .pipelines
            .keys()
            .any(|name| name.starts_with("hybrid") || name.starts_with("reservoir")),
        "reference views must not compile unused live-transport families"
    );
    let resolved = camera.resolve(size)?;
    for (index, encoded) in [128.0f32, 224.0].into_iter().enumerate() {
        let clip = resolved.projection * resolved.view * scene.instances[index].world.w_axis;
        let x = ((clip.x / clip.w * 0.5 + 0.5) * size[0] as f32) as usize;
        let y = ((-clip.y / clip.w * 0.5 + 0.5) * size[1] as f32) as usize;
        let value = radiance[y * size[0] as usize + x];
        let expected = ((encoded / 255.0 + 0.055) / 1.055).powf(2.4) * 3.0;
        assert!(
            (value[index] - expected).abs() < 0.003,
            "instance {index}: {value:?}, expected {expected}"
        );
        assert!(
            value[1 - index].abs() < 1e-5,
            "per-instance material leaked: {value:?}"
        );
    }
    std::fs::create_dir_all(&output)?;
    view.capture(&mut gpu)?
        .save(output.join("native-emissive-uv1.png"))?;
    motion::check(&mut gpu, &mut view, &mut scene, &camera, size, &output)?;
    skin::check(&mut gpu, &mut view, &mut scene, &camera, size, &output)?;
    view.destroy(&mut gpu)?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}

fn native_color_ktx() -> Vec<u8> {
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
    ara_asset_format::write_ktx2(
        8,
        4,
        ara_asset_format::TextureFormat::Bc7Srgb,
        &[[block(128), block(224)].concat()],
    )
    .unwrap()
}
