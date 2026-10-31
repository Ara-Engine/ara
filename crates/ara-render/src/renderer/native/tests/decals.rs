//! Deferred decals: channel blending, receivers, angle and distance fades, draw
//! order and the coverage view, read back from G-buffer captures.
use super::*;

const SIZE: [u32; 2] = [256, 128];

fn quad(key: u8) -> instanced::Geometry {
    let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    instanced::Geometry {
        key: [key; 32],
        vertices: corners
            .into_iter()
            .map(|(x, y)| instanced::Vertex {
                position: [x, y, 0.0],
                normal: [0.0, 0.0, 1.0],
                uv: [0.0; 2],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv1: [0.0; 2],
                terrain_weights: 0,
                color: u32::MAX,
            })
            .collect(),
        lods: vec![instanced::Lod {
            indices: vec![0, 1, 2, 0, 2, 3],
            error: 0.0,
        }],
        bounds: [Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 0.0)],
        deforming: false,
        vertex_colors: false,
        revision: 0,
    }
}

fn material(base: [f32; 4], roughness: f32) -> instanced::Material {
    instanced::Material {
        pbr: crate::scene::Material {
            base_color: base,
            emissive_roughness: [0.0, 0.0, 0.0, roughness],
            textures: [u32::MAX; 4],
            ..Default::default()
        },
        ..Default::default()
    }
}

fn decal(entity: u64, x: f32, material: usize, sort_order: i32) -> instanced::Decal {
    instanced::Decal {
        entity,
        name: format!("decal-{entity}"),
        world: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.0)),
        material,
        opacity: [1.0; 7],
        opacity_channel: 4,
        occlusion: [1.0, 4.0],
        specular: 0.5,
        fade_distance: [0.0, 0.0],
        fade_angle: [60.0, 80.0],
        sort_order,
        fade_screen_size: 0.0,
    }
}

/// Grey quads at x = -3 (receives), 0 (refuses decals) and 3 (receives), and a
/// sideways quad at x = 6 (its normal along +X, across the projection axis). A red
/// decal over each, and a green decal drawn later over the first.
fn scene() -> NativeScene {
    let instance = |entity: u64, world: Mat4, receives_decals: bool| instanced::Instance {
        entity,
        name: format!("quad-{entity}"),
        geometry: 0,
        material: 0,
        world,
        previous_world: world,
        casts_shadows: false,
        paint: None,
        receives_decals,
    };
    let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
    let sideways = at(6.0) * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2);
    NativeScene {
        geometry: vec![quad(9)],
        instances: vec![
            instance(1, at(-3.0), true),
            instance(2, at(0.0), false),
            instance(3, at(3.0), true),
            instance(4, sideways, true),
        ],
        materials: vec![
            material([0.5, 0.5, 0.5, 1.0], 0.8),
            material([1.0, 0.0, 0.0, 1.0], 0.2),
            material([0.0, 1.0, 0.0, 1.0], 0.2),
        ],
        decals: vec![
            decal(10, -3.0, 2, 5),
            decal(11, -3.0, 1, 0),
            decal(12, 0.0, 1, 0),
            decal(13, 3.0, 1, 0),
            decal(14, 6.0, 1, 0),
        ],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::splat(100.0),
        environment: vec![[0.02, 0.03, 0.04, 1.0]],
        environment_size: [1, 1],
    }
}

fn request(mode: ViewMode) -> CaptureRequest {
    CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(1.5, 0.0, 12.0),
            target: Vec3::new(1.5, 0.0, 0.0),
            vertical_fov: 0.6,
        },
        size: SIZE,
        mode,
        deterministic: true,
        exposure: 1.0,
        settle_frames: 1,
        tick: 0,
        seed: 5,
        marks: None,
    }
}

/// Displayed bytes at world point (x, y, 0).
fn at(capture: &crate::capture::Capture, x: f32, y: f32) -> [u8; 3] {
    let projection = Mat4::perspective_rh(0.6, SIZE[0] as f32 / SIZE[1] as f32, 0.1, 100.0)
        * Mat4::look_at_rh(Vec3::new(1.5, 0.0, 12.0), Vec3::new(1.5, 0.0, 0.0), Vec3::Y);
    let clip = projection * Vec4::new(x, y, 0.0, 1.0);
    let px = ((clip.x / clip.w * 0.5 + 0.5) * SIZE[0] as f32) as u32;
    let py = ((0.5 - clip.y / clip.w * 0.5) * SIZE[1] as f32) as u32;
    let p = capture.image.get_pixel(px, py).0;
    [p[0], p[1], p[2]]
}

#[test]
fn decals_blend_receivers_fade_and_order_in_the_gbuffer() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/decals")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let mut scene = scene();
    let mut renderer =
        Renderer::new_native(&scene, Box::new(EmptyTextures), SIZE, &[], root.join("gpu"))?;
    assert!(renderer.capabilities().decals);
    let albedo = renderer.capture(&request(ViewMode::Albedo))?;
    albedo.image.save(root.join("albedo.png"))?;
    let grey = at(&albedo, 0.0, 0.0);
    assert!(grey[0] == grey[1] && grey[0] > 150, "refused decals: {grey:?}");
    assert_eq!(at(&albedo, 3.0, 0.0), [255, 0, 0], "a decal replaces base colour");
    assert_eq!(at(&albedo, -3.0, 0.0), [0, 255, 0], "the higher sort order draws on top");
    assert_eq!(at(&albedo, 3.0, 0.8), grey, "outside the box the surface keeps its colour");
    let rm = renderer.capture(&request(ViewMode::RoughnessMetal))?;
    let rough = at(&rm, 3.0, 0.0)[0];
    // The view shows sRGB-encoded values: 0.2 is 124, 0.8 is 231.
    assert!(rough.abs_diff(124) <= 3, "roughness 0.2 replaces 0.8: {rough}");

    // Distance fade: past the end the decal is gone; the change streams.
    scene.decals[3].fade_distance = [1.0, 2.0];
    renderer.update_native_changed(
        &scene,
        &instanced::Changes {
            decals: true,
            ..Default::default()
        },
    )?;
    let faded = renderer.capture(&request(ViewMode::Albedo))?;
    assert_eq!(at(&faded, 3.0, 0.0), grey, "faded out by distance");

    let coverage = renderer.capture(&request(ViewMode::DecalCoverage))?;
    coverage.image.save(root.join("coverage.png"))?;
    assert_eq!(at(&coverage, -3.0, 0.0), [0, 255, 0], "two decals: green");
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn invalid_decals_are_rejected() {
    let mut bad = scene();
    bad.decals[0].world = Mat4::ZERO;
    assert!(bad.validate().is_err());
    let mut bad = scene();
    bad.decals[0].material = 99;
    assert!(bad.validate().is_err());
    let mut bad = scene();
    bad.decals[0].opacity[1] = 2.0;
    assert!(bad.validate().is_err());
}
