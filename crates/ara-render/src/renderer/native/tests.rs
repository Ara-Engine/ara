use super::*;
use crate::{
    capture::{CaptureRequest, ViewMode},
    instanced,
};

struct EmptyTextures;
impl Textures for EmptyTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut VulkanDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn refine(&mut self, _: &mut VulkanDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn binding(&self) -> Option<instanced::TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut VulkanDevice) -> Result<()> {
        Ok(())
    }
}

struct RejectPublication;
impl Textures for RejectPublication {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, gpu: &mut VulkanDevice) -> Result<TextureStats> {
        EmptyTextures.initialize(gpu)
    }
    fn refine(&mut self, gpu: &mut VulkanDevice) -> Result<TextureStats> {
        EmptyTextures.refine(gpu)
    }
    fn binding(&self) -> Option<instanced::TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn publish_reuse(&mut self, _: &mut dyn Textures) -> Result<()> {
        anyhow::bail!("injected publication rejection")
    }
    fn destroy(self: Box<Self>, _: &mut VulkanDevice) -> Result<()> {
        Ok(())
    }
}

#[test]
fn native_raster_keeps_instances_local_and_tracks_moving_and_skinned_poses_without_rt() -> Result<()>
{
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/native-raster")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let mut scene = NativeScene {
        geometry: vec![instanced::Geometry {
            key: [1; 32],
            vertices: [(-0.5, -0.5), (0.5, -0.5), (0., 0.5)]
                .into_iter()
                .map(|(x, y)| instanced::Vertex {
                    position: [x, y, 0.],
                    normal: [0., 0., 1.],
                    uv: [0., 0.],
                    tangent: [1., 0., 0., 1.],
                    uv1: [1., 0.],
                    terrain_weights: 0,
                    reserved: 0,
                })
                .collect(),
            lods: vec![instanced::Lod {
                indices: vec![0, 1, 2],
                error: 0.,
            }],
            bounds: [Vec3::new(-0.5, -0.5, 0.), Vec3::new(0.5, 0.5, 0.)],
            deforming: true,
            revision: 0,
        }],
        instances: [(-0.65, 1), (0.65, 2)]
            .into_iter()
            .map(|(x, id)| instanced::Instance {
                entity: id,
                name: format!("triangle-{id}"),
                geometry: 0,
                material: 0,
                world: Mat4::from_translation(Vec3::new(x, 0., 0.)),
                previous_world: Mat4::IDENTITY,
            })
            .collect(),
        materials: vec![instanced::Material {
            pbr: crate::scene::Material {
                base_color: [0.8, 0.1, 0.05, 1.],
                textures: [u32::MAX; 4],
                ..Default::default()
            },
            ..Default::default()
        }],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::splat(100.),
        environment: vec![[0.02, 0.03, 0.04, 1.]],
        environment_size: [1, 1],
    };
    scene.materials.push(scene.materials[0]);
    scene
        .instances
        .extend((100..228).map(|entity| instanced::Instance {
            entity,
            name: format!("offscreen-{entity}"),
            geometry: 0,
            material: 1,
            world: Mat4::from_translation(Vec3::new(entity as f32 + 1000., 0., 0.)),
            previous_world: Mat4::IDENTITY,
        }));
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [192, 108],
        &[],
        root.join("cache"),
    )?;
    assert!(
        !renderer.presentation_ready(),
        "fresh targets are not presentable"
    );
    let descriptors = renderer.gpu().descriptor_usage();
    let artifacts = std::mem::replace(&mut renderer.artifacts, root.join("missing-cohort"));
    for _ in 0..80 {
        assert!(
            renderer
                .replace_native_scene(&scene, Box::new(EmptyTextures))
                .is_err()
        );
        assert_eq!(
            renderer.gpu().descriptor_usage(),
            descriptors,
            "failed native upload must retire its buffers"
        );
    }
    renderer.artifacts = artifacts;
    for _ in 0..80 {
        assert!(
            upload::upload_content(
                &mut renderer.gpu,
                environment::snapshot(&scene),
                &root.join("missing-cohort"),
                &renderer.shader_metadata,
                renderer.compute_frame,
            )
            .is_err()
        );
        assert_eq!(
            renderer.gpu().descriptor_usage(),
            descriptors,
            "late IBL failure must retire environment, textures and geometry"
        );
    }
    renderer.gpu().assert_clean()?;
    assert!(!renderer.gpu().context().info.ray_tracing);
    let stats = renderer.native_geometry_stats().unwrap();
    assert_eq!((stats.geometries, stats.blas), (1, 0));
    let request = CaptureRequest {
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
    };
    let first = renderer.capture(&request)?;
    first.write(&root.join("before.png"))?;
    assert_eq!(renderer.entity_at([70, 54])?, Some(1));
    assert_eq!(renderer.entity_at([122, 54])?, Some(2));
    renderer.render_view(&request.camera, 1., ViewMode::Albedo)?;
    let idle = renderer.native_frame_work().unwrap();
    assert_eq!(
        (
            idle.instance_rows,
            idle.material_rows,
            idle.cull_tests,
            idle.lod_tests,
            idle.visible_rows
        ),
        (0, 0, 0, 0, 0)
    );
    scene.instances[1].world.w_axis.x = 0.8;
    let move_only = instanced::Changes {
        instances: vec![1],
        ..Default::default()
    };
    renderer.update_native_changed(&scene, &move_only)?;
    renderer.render_view(&request.camera, 1., ViewMode::Albedo)?;
    let moved = renderer.native_frame_work().unwrap();
    assert!(
        moved.cull_tests > 0 && moved.cull_tests <= 10,
        "only the moved group is culled, not 128 unrelated instances"
    );
    assert_eq!(
        (moved.instance_rows, moved.material_rows, moved.lod_tests),
        (1, 0, 1)
    );
    let dirty = renderer.native_caster_changes().unwrap();
    assert!(!dirty.all);
    assert_eq!(dirty.bounds.len(), 1);
    assert!((dirty.bounds[0][0].x - 0.15).abs() < 1e-6);
    assert!((dirty.bounds[0][1].x - 1.3).abs() < 1e-6);
    // Main and far cull lists are independent; repeated light matrices reuse the list.
    renderer.cull_native_shadow(Mat4::IDENTITY)?;
    let far_tests = renderer.native_frame_work().unwrap().cull_tests;
    renderer.cull_native_shadow(Mat4::IDENTITY)?;
    assert_eq!(renderer.native_frame_work().unwrap().cull_tests, far_tests);
    assert!(
        renderer.native.as_ref().unwrap().culling.draws[6]
            .iter()
            .any(|batch| batch.count > 0)
    );
    renderer.render_view(&request.camera, 1., ViewMode::Albedo)?;
    let settled = renderer.native_frame_work().unwrap();
    assert_eq!(
        (settled.instance_rows, settled.cull_tests, settled.lod_tests),
        (1, 0, 0)
    );
    renderer.render_view(&request.camera, 1., ViewMode::Albedo)?;
    assert_eq!(renderer.native_frame_work().unwrap().instance_rows, 0);
    scene.instances[1].world = Mat4::from_translation(Vec3::new(50., 0., 0.));
    scene.geometry[0].vertices[2].position[1] = 0.25;
    scene.geometry[0].revision = 1;
    renderer.update_native(&scene)?;
    let changed = renderer.capture(&request)?;
    changed.write(&root.join("after.png"))?;
    assert_eq!(renderer.entity_at([122, 54])?, None);
    assert_eq!(renderer.entity_at([70, 54])?, Some(1));
    assert_ne!(first.image, changed.image);
    assert_ne!(first.metadata.scene_hash, changed.metadata.scene_hash);
    assert_eq!(renderer.native_geometry_stats().unwrap().blas, 0);
    assert!(
        renderer
            .timings()
            .iter()
            .all(|pass| !pass.pass.contains("BLAS") && !pass.pass.contains("TLAS"))
    );
    // Authored environments replace both the background and its IBL. Transmission
    // uses the forward blend even for an otherwise opaque glTF material.
    scene.environment[0] = [0.04, 0.3, 0.1, 1.];
    renderer.replace_native_scene(&scene, Box::new(EmptyTextures))?;
    let lit = CaptureRequest {
        mode: ViewMode::Lit,
        settle_frames: 4,
        ..request
    };
    let opaque = renderer.capture(&lit)?;
    let environment = scene.environment[0];
    let previous_environment = renderer.content.static_hash.to_owned();
    let previous_usage = renderer.gpu().descriptor_usage();
    scene.environment[0] = [0.9, 0.02, 0.01, 1.];
    for _ in 0..4 {
        assert!(
            renderer
                .replace_native_scene(&scene, Box::new(RejectPublication))
                .is_err()
        );
        assert_eq!(renderer.content.static_hash, previous_environment);
        assert_eq!(renderer.gpu().descriptor_usage(), previous_usage);
        renderer.render_view(&lit.camera, 1., ViewMode::Lit)?;
        renderer.gpu().assert_clean()?;
    }
    scene.environment[0] = environment;
    for name in ["GBuffer", "TiledLightCull", "DeferredLighting"] {
        assert!(
            renderer.timings().iter().any(|pass| pass.pass == name),
            "batched submissions retain the {name} timestamp"
        );
    }
    scene.materials[0].extras.transmission_factor = 1.;
    renderer.update_native_changed(
        &scene,
        &instanced::Changes {
            materials: vec![0],
            ..Default::default()
        },
    )?;
    let transmitted = renderer.capture(&lit)?;
    transmitted.write(&root.join("transmission.png"))?;
    assert_ne!(opaque.image, transmitted.image);
    assert!(
        renderer
            .native_geometry_stats()
            .is_some_and(|stats| stats.blas == 0)
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
