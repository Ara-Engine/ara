//! Small diagnostic scenes; geometry is shared with every rendering mode.
use ara_render::scene::{CameraSpec, Light, Material, Mesh, Scene, Texture, Vertex, sphere};
use glam::Vec3;
/// Required fidelity verification scenarios.
pub const NAMES: [&str; 7] = [
    "furnace",
    "thin_wall",
    "moving_emissive",
    "time_of_day",
    "disocclusion",
    "glossy_interior",
    "foliage_alpha",
];
/// Fixed physical scene, varying only by explicit simulation time.
pub fn scene(name: &str, tick: u32) -> Scene {
    let mut s = Scene {
        meshes: Vec::new(),
        materials: vec![
            mat([0.7, 0.7, 0.7], 0.65, 0.),
            mat([0.75, 0.12, 0.06], 0.7, 0.),
            mat([0.08, 0.3, 0.7], 0.25, 0.85),
        ],
        textures: vec![
            Texture {
                size: [1, 1],
                rgba: vec![255; 4],
                srgb: false,
            },
            Texture {
                size: [1, 1],
                rgba: vec![128, 128, 255, 255],
                srgb: false,
            },
        ],
        lights: vec![Light {
            position_range: [-1., 2.4, 0., 10.],
            color_lumens: [1., 0.85, 0.65, 900.],
            direction_outer: [0., -1., 0., -1.],
            parameters: [0.; 4],
        }],
        sun_direction: Vec3::new(-0.3, 0.8, 0.5).normalize(),
        sun_lux: Vec3::splat(3.),
        environment: vec![[0.3, 0.4, 0.6, 1.]; 8],
        environment_size: [4, 2],
    };
    if name == "furnace" {
        s.materials = vec![mat([0.8; 3], 0.6, 0.)];
        s.environment = vec![[1.; 4]; 8];
        s.sun_lux = Vec3::ZERO;
        s.lights.clear();
        s.meshes.push(sphere(
            1,
            "furnace sphere".into(),
            Vec3::new(0., 1.2, 0.),
            1.,
            0,
        ));
        return s;
    }
    s.meshes.push(quad(
        1,
        "floor",
        Vec3::new(-3., 0., 3.),
        Vec3::new(6., 0., 0.),
        Vec3::new(0., 0., -6.),
        0,
    ));
    s.meshes.push(quad(
        2,
        "back",
        Vec3::new(-3., 0., -3.),
        Vec3::new(6., 0., 0.),
        Vec3::new(0., 3., 0.),
        0,
    ));
    s.meshes.push(quad(
        3,
        "left",
        Vec3::new(-3., 0., 3.),
        Vec3::new(0., 0., -6.),
        Vec3::new(0., 3., 0.),
        1,
    ));
    s.meshes.push(quad(
        4,
        "right",
        Vec3::new(3., 0., -3.),
        Vec3::new(0., 0., 6.),
        Vec3::new(0., 3., 0.),
        0,
    ));
    s.meshes.push(quad(
        5,
        "ceiling",
        Vec3::new(-3., 3., -3.),
        Vec3::new(6., 0., 0.),
        Vec3::new(0., 0., 6.),
        0,
    ));
    s.meshes.push(sphere(
        6,
        "glossy sphere".into(),
        Vec3::new(0.8, 0.8, -0.5),
        0.8,
        2,
    ));
    s.meshes.push(sphere(
        7,
        "diffuse sphere".into(),
        Vec3::new(-1., 0.65, 0.4),
        0.65,
        0,
    ));
    if name == "thin_wall" {
        s.meshes.push(quad(
            8,
            "two centimetre wall left",
            Vec3::new(-0.01, 0., 1.5),
            Vec3::new(0., 0., -4.5),
            Vec3::new(0., 3., 0.),
            0,
        ));
        s.meshes.push(quad(
            9,
            "two centimetre wall right",
            Vec3::new(0.01, 0., -3.),
            Vec3::new(0., 0., 4.5),
            Vec3::new(0., 3., 0.),
            0,
        ));
        s.sun_lux = Vec3::ZERO;
        s.environment = vec![[0., 0., 0., 1.]; 8];
    }
    if name == "moving_emissive" {
        let mut m = mat([1., 0.7, 0.1], 0.5, 0.);
        m.emissive_roughness = [40., 12., 2., 0.5];
        s.materials.push(m);
        s.meshes.push(sphere(
            10,
            "moving emitter".into(),
            Vec3::new(-1.5 + (tick as f32 * 0.12).sin() * 2., 1.8, -1.),
            0.3,
            3,
        ));
    }
    if name == "time_of_day" {
        let a = tick as f32 * 0.1;
        s.sun_direction = Vec3::new(a.sin(), a.cos().max(0.02), 0.5).normalize();
        s.sun_lux = Vec3::new(18., 12., 7.) * a.cos().max(0.02);
    }
    if name == "disocclusion" {
        s.meshes.push(quad(
            11,
            "moving occluder",
            Vec3::new(-0.6 + (tick as f32 * 0.15).sin() * 2., 0., 1.4),
            Vec3::new(1.2, 0., 0.),
            Vec3::new(0., 2.5, 0.),
            1,
        ));
    }
    if name == "foliage_alpha" {
        let mut rgba = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                rgba.extend([70, 180, 35, if (x / 2 + y / 2) % 2 == 0 { 255 } else { 0 }]);
            }
        }
        s.textures.push(Texture {
            size: [8, 8],
            rgba,
            srgb: true,
        });
        let mut m = mat([1.; 3], 0.8, 0.);
        m.textures[0] = 2;
        m.flags[0] = 1;
        s.materials.push(m);
        s.meshes.push(quad(
            12,
            "masked leaves",
            Vec3::new(-1.6, 0.2, 1.5),
            Vec3::new(3.2, 0., 0.),
            Vec3::new(0., 2.4, 0.),
            3,
        ));
    }
    s
}
/// Fixed view shared by hybrid and reference captures.
pub fn camera() -> CameraSpec {
    CameraSpec::LookAt {
        position: Vec3::new(0., 1.6, 6.5),
        target: Vec3::new(0., 1.2, -0.4),
        vertical_fov: 0.9,
    }
}
/// Fixture surface with linear albedo, GGX roughness and metalness.
pub(super) fn mat(colour: [f32; 3], roughness: f32, metal: f32) -> Material {
    Material {
        base_color: [colour[0], colour[1], colour[2], 1.],
        emissive_roughness: [0., 0., 0., roughness],
        surface: [metal, 1., 0., 0.5],
        ..Material::default()
    }
}
/// Fixture quad with winding and vertex normals along u cross v.
pub(super) fn quad(
    entity: u64,
    name: &str,
    origin: Vec3,
    u: Vec3,
    v: Vec3,
    material: usize,
) -> Mesh {
    let normal = u.cross(v).normalize();
    let tangent = u.normalize();
    let vertices = [
        (origin, [0., 0.]),
        (origin + u, [1., 0.]),
        (origin + u + v, [1., 1.]),
        (origin + v, [0., 1.]),
    ]
    .map(|(p, uv)| Vertex {
        position: p.to_array(),
        normal: normal.to_array(),
        uv,
        tangent: tangent.extend(1.).to_array(),
    })
    .to_vec();
    Mesh {
        entity,
        name: name.into(),
        vertices,
        indices: vec![0, 1, 2, 0, 2, 3],
        material,
    }
}
