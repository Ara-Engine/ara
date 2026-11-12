use super::*;
use crate::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    instanced::*,
    scene::CameraSpec,
};
use anyhow::Result;
use ara_gpu::{Access, GpuDevice, Resource};
use glam::{Mat4, Vec3};

struct EmptyTextures;
impl Textures for EmptyTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn refine(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn binding(&self) -> Option<TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut GpuDevice) -> Result<()> {
        Ok(())
    }
}

fn scene() -> crate::instanced::Scene {
    let mut mirror = Material::default();
    mirror.pbr.emissive_roughness = [0., 0., 0., 0.];
    mirror.pbr.surface[0] = 1.;
    let mut emitter = Material::default();
    emitter.pbr.base_color = [0., 0., 0., 1.];
    emitter.pbr.emissive_roughness = [8., 0., 0., 1.];
    let geometry = Geometry {
        key: [184; 32],
        revision: 0,
        deforming: false,
        vertex_colors: false,
        vertices: [[-1., -1., 0.], [1., -1., 0.], [1., 1., 0.], [-1., 1., 0.]]
            .map(|position| Vertex {
                position,
                normal: [0., 0., 1.],
                tangent: [1., 0., 0., 1.],
                uv: [0.; 2],
                uv1: [0.; 2],
                terrain_weights: 0,
                color: Vertex::WHITE,
            })
            .to_vec(),
        lods: vec![Lod {
            indices: vec![0, 1, 2, 0, 2, 3],
            error: 0.,
        }],
        bounds: [Vec3::new(-1., -1., 0.), Vec3::new(1., 1., 0.)],
    };
    let instances = [
        Mat4::IDENTITY,
        Mat4::from_translation(Vec3::new(0., 0., 6.))
            * Mat4::from_rotation_y(std::f32::consts::PI)
            * Mat4::from_scale(Vec3::splat(4.)),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, world)| Instance {
        entity: i as u64 + 1,
        name: format!("quad-{i}"),
        geometry: 0,
        material: i,
        world,
        previous_world: world,
        casts_shadows: true,
        paint: None,
        receives_decals: true,
    })
    .collect();
    crate::instanced::Scene {
        geometry: vec![geometry],
        instances,
        materials: vec![mirror, emitter],
        textures: 0,
        decals: vec![],
        lights: vec![],
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ZERO,
        environment: vec![[0.02, 0.02, 0.02, 1.]],
        environment_size: [1, 1],
    }
}

fn settings() -> SoftwareReflections {
    SoftwareReflections {
        mirror_cut: 0.1,
        rough_cut: 0.5,
        rough_rays: 4,
        distance: 100.,
        interfaces: 8,
    }
}

#[test]
fn rejects_nonfinite_and_inverted_experiment_cuts() {
    let mut s = settings();
    assert!(s.validate().is_ok());
    s.mirror_cut = f32::NAN;
    assert!(s.validate().is_err());
    s = settings();
    s.rough_cut = 0.;
    assert!(s.validate().is_err());
    s = settings();
    s.rough_rays = 0;
    assert!(s.validate().is_err());
}

#[test]
fn software_reflections_find_offscreen_emission_and_disable_byte_identically() -> Result<()> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/reflect-tests")
        .join(std::process::id().to_string());
    let mut scene = scene();
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    renderer.set_quality(crate::look::Quality {
        high_tier: true,
        ..Default::default()
    })?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 0., 3.),
            target: Vec3::ZERO,
            vertical_fov: 0.6,
        },
        size: [96, 64],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 2,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let control = renderer.capture(&request)?;
    assert!(
        !renderer
            .timings()
            .iter()
            .any(|t| t.pass.starts_with("Reflect."))
    );
    renderer.set_software_reflections(Some(settings()), &scene)?;
    let candidate = renderer.capture(&request)?;
    let centre = candidate.image.get_pixel(48, 32);
    assert!(
        centre[0] > 200 && centre[1] < 60,
        "offscreen red emitter: {centre:?}"
    );
    let counters = renderer.software_reflection_counters()?.unwrap();
    assert!(counters.mirror_pixels > 0 && counters.rays >= counters.mirror_pixels);
    assert_eq!(counters.exhausted, 0);
    let repeated = renderer.capture(&request)?;
    assert_eq!(candidate.image, repeated.image);
    scene.materials[1].pbr.flags[0] = 1;
    scene.materials[1].pbr.base_color[3] = 0.;
    scene.materials[1].pbr.surface[3] = 0.5;
    renderer.replace_native_scene(&scene, Box::new(EmptyTextures))?;
    let cutout = renderer.capture(&request)?;
    assert!(
        cutout.image.get_pixel(48, 32)[0] < 60,
        "native alpha factor must reject the offscreen emitter"
    );
    scene.materials[1].pbr.flags[0] = 0;
    scene.materials[1].pbr.base_color[3] = 1.;
    renderer.replace_native_scene(&scene, Box::new(EmptyTextures))?;
    scene.instances[1].world = Mat4::from_translation(Vec3::new(30., 0., 6.));
    renderer.update_native(&scene)?;
    renderer.render(&request.camera, request.exposure)?;
    let moved = renderer.capture_completed(&request)?;
    assert!(
        moved.image.get_pixel(48, 32)[0] < 60,
        "motion refit must remove the emitter on the first live frame, without history"
    );
    scene.instances[1].world = Mat4::from_translation(Vec3::new(0., 0., 6.))
        * Mat4::from_rotation_y(std::f32::consts::PI)
        * Mat4::from_scale(Vec3::splat(4.));
    renderer.update_native(&scene)?;
    renderer.set_software_reflections(None, &scene)?;
    let restored = renderer.capture(&request)?;
    assert_eq!(control.image, restored.image);
    std::fs::create_dir_all(&root)?;
    candidate.image.save(root.join("offscreen.png"))?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn ordered_glass_sees_the_emitter_behind_two_interfaces() -> Result<()> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/reflect-glass-tests")
        .join(std::process::id().to_string());
    let mut scene = scene();
    scene.materials[0].pbr.surface[0] = 0.;
    scene.materials[0].pbr.base_color = [0.4, 0.7, 0.9, 1.];
    scene.materials[0].pbr.flags[0] = 2;
    scene.materials[0].extras.transmission_factor = 1.;
    scene.materials[1].pbr.surface[0] = 1.;
    scene.materials[1].pbr.emissive_roughness = [1., 1., 1., 1.];
    scene.instances[1].world =
        Mat4::from_translation(Vec3::new(0., 0., -6.)) * Mat4::from_scale(Vec3::splat(4.));
    scene.instances.push(Instance {
        entity: 3,
        name: "second pane".into(),
        geometry: 0,
        material: 0,
        world: Mat4::from_translation(Vec3::new(0., 0., -1.)),
        previous_world: Mat4::IDENTITY,
        casts_shadows: true,
        paint: None,
        receives_decals: true,
    });
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    renderer.set_quality(crate::look::Quality {
        high_tier: true,
        ..Default::default()
    })?;
    renderer.set_software_reflections(Some(settings()), &scene)?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 0., 3.),
            target: Vec3::ZERO,
            vertical_fov: 0.6,
        },
        size: [96, 64],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 2,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let capture = renderer.capture(&request)?;
    let c = capture.image.get_pixel(48, 32);
    assert!(
        c[0] + 25 < c[1] && c[1] + 10 < c[2],
        "both tinted interfaces: {c:?}"
    );
    let work = renderer.software_reflection_counters()?.unwrap();
    assert!(work.glass_interfaces > 0 && work.rays > work.glass_interfaces);
    assert_eq!(work.invalid_media, 0);
    std::fs::create_dir_all(&root)?;
    capture.image.save(root.join("ordered-glass.png"))?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn secondary_local_visibility_uses_any_hit_outside_the_screen() -> Result<()> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/reflect-visibility-tests")
        .join(std::process::id().to_string());
    let mut scene = scene();
    scene.environment = vec![[0., 0., 0., 1.]];
    scene.materials[1].pbr.base_color = [0.7, 0.7, 0.7, 1.];
    scene.materials[1].pbr.emissive_roughness = [0., 0., 0., 1.];
    scene.lights.push(ara_frame::scene::Light {
        position_range: [2., 0., 4., 20.],
        color_lumens: [1., 0., 0., 400.],
        direction_outer: [0., 0., -1., -1.],
        parameters: [0., 1., 0., 1.],
        ..ara_frame::scene::Light::ZERO
    });
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    renderer.set_quality(crate::look::Quality {
        high_tier: true,
        local_shadows: None,
        ..Default::default()
    })?;
    renderer.set_software_reflections(Some(settings()), &scene)?;
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 0., 3.),
            target: Vec3::ZERO,
            vertical_fov: 0.6,
        },
        size: [96, 64],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 2,
        tick: 0,
        seed: 0,
        marks: None,
    };
    let lit = renderer.capture(&request)?;
    assert!(lit.image.get_pixel(48, 32)[0] > 80);
    let work = renderer.software_reflection_counters()?.unwrap();
    assert!(work.visibility_rays > 0);
    scene.instances.push(Instance {
        entity: 3,
        name: "secondary blocker".into(),
        geometry: 0,
        material: 0,
        world: Mat4::from_translation(Vec3::new(1., 0., 5.)) * Mat4::from_scale(Vec3::splat(0.3)),
        previous_world: Mat4::IDENTITY,
        casts_shadows: true,
        paint: None,
        receives_decals: true,
    });
    renderer.replace_native_scene(&scene, Box::new(EmptyTextures))?;
    let shadow = renderer.capture(&request)?;
    assert!(
        shadow.image.get_pixel(48, 32)[0] < 20,
        "offscreen secondary blocker: {:?}",
        shadow.image.get_pixel(48, 32)
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
