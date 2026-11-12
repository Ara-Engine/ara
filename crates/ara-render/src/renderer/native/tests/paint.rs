//! Vertex colours: the glTF tint, per-instance paint, vertex-paint layers and the
//! vertex-colour views, read back from Albedo/VertexColor captures.
use super::*;
use ara_frame::scene::{PaintWeight, VERTEX_COLOR_TINT, VERTEX_PAINT_LAYERS};

const SIZE: [u32; 2] = [256, 128];
const GEOMETRY_COLOR: [f32; 4] = [1.0, 0.5, 0.25, 1.0];

fn material(base: [f32; 3], flags: u32) -> instanced::Material {
    instanced::Material {
        pbr: ara_frame::scene::Material {
            base_color: [base[0], base[1], base[2], 1.],
            textures: [u32::MAX; 4],
            flags: [0, flags, 0, 0],
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Four quads in a row at x = -4.5, -1.5, 1.5, 4.5 sharing one coloured geometry:
/// untinted, tinted, tinted with green paint, and a red/blue paint-layer blend by R.
fn scene() -> NativeScene {
    let color = instanced::pack_color(GEOMETRY_COLOR);
    let mut layered = material([1.; 3], VERTEX_PAINT_LAYERS);
    layered.extras.terrain_layers = 2;
    layered.extras.layer_materials = [2, 3, u32::MAX, u32::MAX];
    use PaintWeight::*;
    layered.pbr.flags[2] = u32::from(PaintWeight::pack([Inverse, Unused, Unused, Unused]))
        | u32::from(PaintWeight::pack([Channel, Unused, Unused, Unused])) << 8;
    let quad = [(-0.75, -0.75), (0.75, -0.75), (0.75, 0.75), (-0.75, 0.75)];
    let instance =
        |entity: u64, x: f32, material: usize, paint: Option<[f32; 4]>| instanced::Instance {
            entity,
            name: format!("quad-{entity}"),
            geometry: 0,
            material,
            world: Mat4::from_translation(Vec3::new(x, 0., 0.)),
            previous_world: Mat4::from_translation(Vec3::new(x, 0., 0.)),
            casts_shadows: false,
            paint: paint.map(|c| vec![instanced::pack_color(c); 4].into()),
            receives_decals: true,
        };
    NativeScene {
        geometry: vec![instanced::Geometry {
            key: [7; 32],
            vertices: quad
                .into_iter()
                .map(|(x, y)| instanced::Vertex {
                    position: [x, y, 0.],
                    normal: [0., 0., 1.],
                    uv: [0.; 2],
                    tangent: [1., 0., 0., 1.],
                    uv1: [0.; 2],
                    terrain_weights: 0,
                    color,
                })
                .collect(),
            lods: vec![instanced::Lod {
                indices: vec![0, 1, 2, 0, 2, 3],
                error: 0.,
            }],
            bounds: [Vec3::new(-0.75, -0.75, 0.), Vec3::new(0.75, 0.75, 0.)],
            deforming: false,
            vertex_colors: true,
            revision: 0,
        }],
        instances: vec![
            instance(1, -4.5, 0, None),
            instance(2, -1.5, 1, None),
            instance(3, 1.5, 1, Some([0., 1., 0., 1.])),
            instance(4, 4.5, 4, None),
        ],
        materials: vec![
            material([0.5; 3], 0),
            material([1.; 3], VERTEX_COLOR_TINT),
            material([0.8, 0.1, 0.1], 0),
            material([0.1, 0.1, 0.8], 0),
            layered,
        ],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::splat(100.),
        environment: vec![[0.02, 0.03, 0.04, 1.]],
        environment_size: [1, 1],
        decals: Vec::new(),
    }
}

fn request(mode: ViewMode) -> CaptureRequest {
    CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 0., 8.),
            target: Vec3::ZERO,
            vertical_fov: 0.8,
        },
        size: SIZE,
        mode,
        deterministic: true,
        exposure: 1.,
        settle_frames: 1,
        tick: 0,
        seed: 3,
        marks: None,
    }
}

/// The displayed sRGB bytes at quad `n`'s centre.
fn at(capture: &ara_frame::capture::Capture, n: usize) -> [u8; 3] {
    let x = -4.5 + 3. * n as f32;
    let projection = Mat4::perspective_rh(0.8, SIZE[0] as f32 / SIZE[1] as f32, 0.1, 100.)
        * Mat4::look_at_rh(Vec3::new(0., 0., 8.), Vec3::ZERO, Vec3::Y);
    let clip = projection * Vec4::new(x, 0., 0., 1.);
    let px = ((clip.x / clip.w * 0.5 + 0.5) * SIZE[0] as f32) as u32;
    let py = ((0.5 - clip.y / clip.w * 0.5) * SIZE[1] as f32) as u32;
    let p = capture.image.get_pixel(px, py).0;
    [p[0], p[1], p[2]]
}

fn srgb(linear: f32) -> u8 {
    let c = linear.clamp(0., 1.);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1. / 2.4) - 0.055
    };
    (s * 255. + 0.5) as u8
}

fn near(actual: [u8; 3], expected: [u8; 3], what: &str) {
    assert!(
        actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 3),
        "{what}: {actual:?}, expected {expected:?}"
    );
}

#[test]
fn vertex_colours_tint_paint_and_blend_layers_per_instance() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/vertex-paint")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let mut scene = scene();
    let mut renderer =
        Renderer::new_native(&scene, Box::new(EmptyTextures), SIZE, &[], root.join("gpu"))?;
    {
        let native = renderer.native.as_ref().unwrap();
        let paint =
            |entity: usize| native.groups.groups[native.groups.instance_group[entity]].paint;
        // The untinted material keeps the plain PSO; the rest read vertex colours.
        assert_eq!([0, 1, 2, 3].map(paint), [false, true, true, true]);
    }
    let albedo = renderer.capture(&request(ViewMode::Albedo))?;
    albedo.image.save(root.join("albedo.png"))?;
    let rgb = |c: [f32; 3]| c.map(srgb);
    near(
        at(&albedo, 0),
        rgb([0.5; 3]),
        "an untinted material ignores colours",
    );
    near(
        at(&albedo, 1),
        rgb([1., 0.5, 0.25]),
        "COLOR_0 multiplies base colour",
    );
    near(
        at(&albedo, 2),
        rgb([0., 1., 0.]),
        "paint replaces this instance's colours",
    );
    near(
        at(&albedo, 3),
        rgb([0.1, 0.1, 0.8]),
        "R = 1 selects the second layer",
    );

    let colours = renderer.capture(&request(ViewMode::VertexColor))?;
    near(
        at(&colours, 0),
        [255, 128, 64],
        "the view shows stored values",
    );
    near(at(&colours, 2), [0, 255, 0], "the view shows paint");
    let red = renderer.capture(&request(ViewMode::VertexColorRed))?;
    near(at(&red, 1), [255; 3], "red channel");
    near(at(&red, 2), [0; 3], "painted red channel");
    let green = renderer.capture(&request(ViewMode::VertexColorGreen))?;
    near(at(&green, 1), [128; 3], "green channel");

    // A live paint edit streams into this instance's range only.
    scene.instances[2].paint = Some(vec![instanced::pack_color([0., 0., 1., 1.]); 4].into());
    scene.instances[3].paint = None;
    let changes = instanced::Changes {
        paint: vec![2],
        ..Default::default()
    };
    renderer.update_native_changed(&scene, &changes)?;
    let edited = renderer.capture(&request(ViewMode::Albedo))?;
    near(at(&edited, 2), rgb([0., 0., 1.]), "the edited instance");
    near(
        at(&edited, 1),
        rgb([1., 0.5, 0.25]),
        "the shared geometry stays",
    );
    // Adding paint regroups: a structural replacement.
    scene.instances[3].paint = Some(vec![instanced::pack_color([0., 0., 0., 1.]); 4].into());
    assert!(
        renderer
            .update_native_changed(
                &scene,
                &instanced::Changes {
                    paint: vec![3],
                    ..Default::default()
                }
            )
            .is_err(),
        "paint presence is part of an instance's grouping"
    );
    renderer.replace_native_scene(&scene, Box::new(EmptyTextures))?;
    let replaced = renderer.capture(&request(ViewMode::Albedo))?;
    near(
        at(&replaced, 3),
        rgb([0.8, 0.1, 0.1]),
        "R = 0 selects the first layer",
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn validation_rejects_undeclared_colours_and_short_paint() {
    let mut scene = scene();
    scene.geometry[0].vertex_colors = false;
    assert!(scene.validate().is_err(), "colours must be declared");
    let mut scene = self::scene();
    scene.instances[2].paint = Some(vec![u32::MAX; 3].into());
    assert!(
        scene.validate().is_err(),
        "paint needs one colour per vertex"
    );
    assert!(self::scene().validate().is_ok());
}

#[test]
fn paint_weights_flatten_a_lerp_tree() {
    use PaintWeight::*;
    let first = PaintWeight::pack([Inverse, Inverse, Unused, Unused]);
    let second = PaintWeight::pack([Channel, Inverse, Unused, Unused]);
    let third = PaintWeight::pack([Unused, Channel, Unused, Unused]);
    let rgba = [0.25, 0.5, 0., 1.];
    let weights = [first, second, third].map(|byte| PaintWeight::weight(byte, rgba));
    // lerp(lerp(a, b, r), c, g): a (1-r)(1-g), b r(1-g), c g.
    assert_eq!(weights, [0.375, 0.125, 0.5]);
    assert_eq!(weights.iter().sum::<f32>(), 1.);
}
