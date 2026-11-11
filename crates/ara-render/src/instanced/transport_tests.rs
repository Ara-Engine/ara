use crate::instanced::transport::{flags, BodyClass, NativeChangeFeed};
use crate::instanced::{
    Changes, Geometry, Instance, Lod, Material, Scene, Vertex,
};
use anyhow::Result;
use glam::{Mat4, Vec3};

fn scene() -> Scene {
    Scene {
        geometry: vec![Geometry {
            key: [1; 32],
            vertices: vec![Vertex {
                position: [0.; 3],
                normal: [0., 1., 0.],
                uv: [0.; 2],
                tangent: [1., 0., 0., 1.],
                uv1: [0.; 2],
                terrain_weights: 0,
                color: u32::MAX,
            }],
            lods: vec![Lod {
                indices: vec![0, 0, 0],
                error: 0.,
            }],
            bounds: [Vec3::splat(-0.5), Vec3::splat(0.5)],
            deforming: true,
            vertex_colors: false,
            revision: 0,
        }],
        instances: (0..2)
            .map(|entity| Instance {
                entity,
                name: String::new(),
                geometry: 0,
                material: 0,
                world: Mat4::IDENTITY,
                previous_world: Mat4::IDENTITY,
                casts_shadows: true,
                paint: None,
                receives_decals: true,
            })
            .collect(),
        materials: vec![Material::default()],
        textures: 0,
        decals: vec![],
        lights: vec![crate::scene::Light::ZERO],
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ONE,
        environment: vec![],
        environment_size: [0, 0],
    }
}
#[test]
fn native_changes_share_deformation_and_preserve_legacy_rows() -> Result<()> {
    let mut scene = scene();
    let mut feed = NativeChangeFeed::new(&scene, 0)?;
    feed.finish();
    let mut changes = Changes {
        geometries: vec![0],
        instances: vec![1],
        ..Default::default()
    };
    changes.transport.deformations.push((0, 0.25));
    scene.geometry[0].revision = 1;
    scene.instances[1].world = Mat4::from_translation(Vec3::X);
    feed.update(&scene, Some(&changes))?;
    let rows = feed.finish();
    assert_eq!(rows.instances.len(), 2);
    assert_eq!(rows.geometries.len(), 1);
    assert!(rows.instances[0].displacement >= 0.25);
    assert!(rows.instances[1].displacement >= 1.25);
    assert_eq!(changes.geometries, [0]);
    assert_eq!(changes.instances, [1]);
    changes.clear();
    feed.update(&scene, Some(&changes))?;
    assert!(feed.finish().instances.is_empty());
    Ok(())
}
#[test]
fn native_metadata_survives_idle_frames_and_transitions() -> Result<()> {
    let scene = scene();
    let mut feed = NativeChangeFeed::new(&scene, 0)?;
    feed.finish();
    let mut changes = Changes::default();
    changes.transport.bodies.push((0, BodyClass::Sleeping));
    changes.transport.wind.push((1, 0.3));
    feed.update(&scene, Some(&changes))?;
    assert_ne!(
        feed.finish().instance(0).unwrap().change.flags & flags::SLEEP,
        0
    );
    changes.clear();
    changes.transport.bodies.push((0, BodyClass::PhysicsActive));
    feed.update(&scene, Some(&changes))?;
    let rows = feed.finish();
    assert_ne!(rows.instance(0).unwrap().change.flags & flags::WAKE, 0);
    assert!(rows.instance(1).unwrap().displacement >= 0.6);
    changes.clear();
    feed.update(&scene, Some(&changes))?;
    assert_eq!(feed.finish().instances.len(), 1);
    Ok(())
}
#[test]
fn native_publication_and_light_identity_changes_keep_stamps() -> Result<()> {
    let mut scene = scene();
    let mut feed = NativeChangeFeed::new(&scene, 0)?;
    feed.finish();
    let mut c = Changes::default();
    c.transport.light_ids.push(500);
    feed.update(&scene, Some(&c))?;
    let rows = feed.finish();
    assert_eq!(rows.lights.len(), 2);
    assert_eq!(rows.lights[1].id, 500);
    scene.lights[0].color_lumens[3] = 10.;
    c.clear();
    feed.update(&scene, Some(&c))?;
    assert_eq!(feed.finish().lights[0].id, 500);
    feed.replace(&scene)?;
    let rows = feed.finish();
    assert!(rows.reset);
    assert_eq!(rows.instance(0).unwrap().change.generation, 2);
    Ok(())
}
#[test]
fn native_multiple_uploads_share_one_frame_and_failed_hints_are_atomic() -> Result<()> {
    let mut scene = scene();
    let mut feed = NativeChangeFeed::new(&scene, 0)?;
    feed.finish();
    feed.begin_frame(10)?;
    let mut c = Changes {
        instances: vec![0],
        ..Default::default()
    };
    scene.instances[0].world = Mat4::from_translation(Vec3::X);
    feed.update(&scene, Some(&c))?;
    scene.instances[0].world = Mat4::from_translation(Vec3::X * 2.);
    feed.update(&scene, Some(&c))?;
    let rows = feed.finish();
    assert_eq!(rows.frame, 10);
    assert_eq!(rows.instances.len(), 1);
    assert!(rows.instances[0].displacement >= 2.);
    c.transport.wind.push((999, 1.));
    assert!(feed.update(&scene, Some(&c)).is_err());
    assert_eq!(feed.finish().frame, 10);
    Ok(())
}

#[test]
fn native_snapshot_detects_paint_and_profile_payload_changes_without_false_idle_events()
-> Result<()> {
    let mut scene = scene();
    scene.instances[0].paint = Some(vec![u32::MAX].into());
    let mut feed = NativeChangeFeed::new(&scene, 0)?;
    feed.finish();
    feed.update(&scene, None)?;
    assert!(feed.finish().instances.is_empty());
    scene.instances[0].paint = Some(vec![0x11223344].into());
    feed.update(&scene, None)?;
    assert_eq!(
        feed.finish().instance(0).unwrap().change.flags,
        flags::PAINT
    );
    let mut changes = Changes::default();
    changes.transport.reshaped_lights.push(0);
    feed.update(&scene, Some(&changes))?;
    assert_eq!(feed.finish().lights[0].flags, flags::SHAPE);
    changes.clear();
    feed.update(&scene, Some(&changes))?;
    assert!(feed.finish().lights.is_empty());
    Ok(())
}

struct EmptyTextures;
impl crate::instanced::Textures for EmptyTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut ara_gpu::GpuDevice) -> Result<crate::instanced::TextureStats> {
        Ok(Default::default())
    }
    fn refine(&mut self, _: &mut ara_gpu::GpuDevice) -> Result<crate::instanced::TextureStats> {
        Ok(Default::default())
    }
    fn binding(&self) -> Option<crate::instanced::TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(ara_gpu::Resource, ara_gpu::Access)) {}
    fn destroy(self: Box<Self>, _: &mut ara_gpu::GpuDevice) -> Result<()> {
        Ok(())
    }
}

#[test]
fn native_feed_capture_is_byte_identical_and_renderer_accepts_replay() -> Result<()> {
    let mut scene = scene();
    scene.instances.truncate(1);
    scene.instances[0].entity = 12;
    scene.geometry[0].vertices = [[-0.5, -0.5, 0.], [0.5, -0.5, 0.], [0., 0.5, 0.]]
        .map(|position| Vertex {
            position,
            normal: [0., 0., 1.],
            uv: [0.; 2],
            tangent: [1., 0., 0., 1.],
            uv1: [0.; 2],
            terrain_weights: 0,
            color: u32::MAX,
        })
        .to_vec();
    scene.geometry[0].lods[0].indices = vec![0, 1, 2];
    scene.lights.clear();
    scene.environment = vec![[0.02, 0.03, 0.04, 1.]];
    scene.environment_size = [1, 1];
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/a3-feed")
        .join(std::process::id().to_string());
    let mut renderer = crate::Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [96, 64],
        &[],
        root.join("cache"),
    )?;
    let request = crate::capture::CaptureRequest {
        camera: crate::scene::CameraSpec::LookAt {
            position: Vec3::new(0., 0., 3.),
            target: Vec3::ZERO,
            vertical_fov: 0.8,
        },
        size: [96, 64],
        mode: crate::capture::ViewMode::Albedo,
        deterministic: true,
        exposure: 1.,
        settle_frames: 1,
        tick: 0,
        seed: 7,
        marks: None,
    };
    let baseline = renderer.capture(&request)?;
    renderer.enable_native_change_feed(&scene, 0)?;
    let initial = renderer.native_transport_changes().unwrap().clone();
    let with_feed = renderer.capture(&request)?;
    assert_eq!(baseline.image.as_raw(), with_feed.image.as_raw());
    renderer.begin_native_change_frame(1)?;
    scene.instances[0].world.w_axis.x = 0.2;
    renderer.update_native_changed(
        &scene,
        &Changes {
            instances: vec![0],
            ..Default::default()
        },
    )?;
    let movement = renderer.native_transport_changes().unwrap().clone();
    assert!(movement.instance(0).unwrap().displacement >= 0.2);
    let moved_with_feed = renderer.capture(&request)?;
    renderer.replace_native_scene(&scene, Box::new(EmptyTextures))?;
    let replaced = renderer.native_transport_changes().unwrap();
    assert!(replaced.reset);
    assert_eq!(replaced.instance(0).unwrap().change.generation, 3);
    renderer.replay_native_transport_frame(&initial)?;
    assert_eq!(renderer.native_transport_changes().unwrap(), &initial);
    renderer.update_native_changed(&scene, &Changes::default())?;
    renderer.replay_native_transport_frame(&movement)?;
    assert_eq!(renderer.native_transport_changes().unwrap(), &movement);
    renderer.disable_native_change_feed();
    let moved_control = renderer.capture(&request)?;
    assert_eq!(moved_with_feed.image.as_raw(), moved_control.image.as_raw());
    renderer.gpu().assert_clean()?;
    Ok(())
}
