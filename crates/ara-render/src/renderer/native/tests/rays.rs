use super::*;
use crate::look::{Quality, RayQueries};

/// A 2 m mirror at the origin facing the camera and a red emitter behind the camera
/// facing the mirror: only a traced ray can see the emitter (screen-space
/// reflections never leave the screen).
fn mirror_scene() -> NativeScene {
    let mut mirror = instanced::Material::default();
    mirror.pbr.emissive_roughness = [0., 0., 0., 0.];
    mirror.pbr.surface[0] = 1.;
    let mut emitter = instanced::Material::default();
    emitter.pbr.base_color = [0., 0., 0., 1.];
    emitter.pbr.emissive_roughness = [8., 0., 0., 1.];
    let quad = |key| instanced::Geometry {
        key: [key; 32],
        revision: 0,
        deforming: false,
        vertices: [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)]
            .into_iter()
            .map(|(x, y)| instanced::Vertex {
                position: [x, y, 0.],
                normal: [0., 0., 1.],
                tangent: [1., 0., 0., 1.],
                uv: [0.; 2],
                uv1: [0.; 2],
                terrain_weights: 0,
                reserved: 0,
            })
            .collect(),
        lods: vec![instanced::Lod {
            indices: vec![0, 1, 2, 0, 2, 3],
            error: 0.,
        }],
        bounds: [Vec3::new(-1., -1., 0.), Vec3::new(1., 1., 0.)],
    };
    let instance = |i: usize, world| instanced::Instance {
        entity: i as u64,
        name: format!("quad-{i}"),
        geometry: 0,
        material: i,
        world,
        previous_world: world,
    };
    NativeScene {
        geometry: vec![quad(29)],
        materials: vec![mirror, emitter],
        instances: vec![
            instance(0, Mat4::IDENTITY),
            instance(
                1,
                Mat4::from_translation(Vec3::new(0., 0., 6.))
                    * Mat4::from_rotation_y(std::f32::consts::PI)
                    * Mat4::from_scale(Vec3::splat(4.)),
            ),
        ],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ONE,
        environment: vec![[0.02, 0.02, 0.02, 1.]],
        environment_size: [1, 1],
    }
}

#[test]
fn hybrid_reflections_see_what_the_screen_cannot_and_raster_presets_trace_nothing() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/hybrid-rays")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let scene = mirror_scene();
    let mut renderer = Renderer::new_hybrid(
        environment::snapshot(&scene),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    if !renderer.gpu().info().ray_tracing {
        eprintln!("skipped: the device has no hardware ray queries");
        return Ok(());
    }
    renderer.replace_native_scene(&scene, Box::new(EmptyTextures))?;
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
        seed: 7,
        marks: None,
    };
    let traced = |renderer: &Renderer| renderer.timings().iter().any(|t| t.pass == "RT.Reflect");
    let centre = |capture: &crate::capture::Capture| *capture.image.get_pixel(48, 32);
    let hybrid = Quality {
        high_tier: true,
        ray_queries: Some(RayQueries {
            reflection_roughness: 0.3,
            distance: 100.,
        }),
        ..Quality::default()
    };
    renderer.set_quality(hybrid)?;
    let mirrored = renderer.capture(&request)?;
    assert!(traced(&renderer), "hybrid frames trace reflections");
    renderer.set_quality(Quality {
        ray_queries: None,
        ..hybrid
    })?;
    let raster = renderer.capture(&request)?;
    assert!(!traced(&renderer), "raster frames declare no ray pass");
    let (m, r) = (centre(&mirrored), centre(&raster));
    assert!(
        m[0] > 200 && m[1] < 60 && m[2] < 60,
        "the mirror reflects the red emitter behind the camera: {m:?}"
    );
    assert!(r[0] < 60, "raster cannot see behind the camera: {r:?}");
    renderer.gpu().assert_clean()?;
    Ok(())
}
