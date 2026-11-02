//! The G2/G4 shadow fixtures: T1's lab catalogue (`ara-dev` `lab/stress/cases.v1.json`,
//! read as-is) and its recipes (`render-fidelity` example `render_v3/fixture.rs`) for every
//! G2 and G4 case, rebuilt here on the public renderer API with the same geometry, motion,
//! lights, sun script and camera; plus B2's own city-scale stress (`city-*`, not in the
//! catalogue). Rect-light and point-light recipes shade through local lights, not sun
//! pages: they run today's control only.
use super::contract;
use ara_render::instanced::{Geometry, Instance, Lod, Material, Scene, Vertex};
use ara_render::scene::{CameraSpec, Light};
use glam::{Mat4, Quat, Vec3};

/// One case: the catalogue's fields this bench reads, and the catalogue entry itself
/// (`None` for B2's own cases), which lab receipts carry unchanged.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Case {
    pub id: String,
    pub gates: Vec<u8>,
    pub recipe: String,
    pub frames: u32,
    pub event_frame: u32,
    pub bodies: u32,
    pub triangles: u32,
    pub lights: u32,
    pub range_m: f32,
    pub sun_deg_s: f32,
    pub elevation_deg: f32,
    pub contact_m: f32,
    #[serde(skip)]
    pub catalogue: Option<contract::Case>,
}

/// Recipes this bench builds.
const RECIPES: [&str; 16] = [
    "sun",
    "pole_sweep",
    "wind",
    "avalanche",
    "collapse",
    "wall",
    "whip_pan",
    "flythrough",
    "contact_sun",
    "contact_rect",
    "overlap",
    "crate_gap",
    "hand",
    "stained_glass",
    "rect_orientation",
    "eight_lights",
];

/// Every G2 and G4 case of T1's catalogue, in catalogue order, then B2's city stress.
pub(crate) fn cases() -> Vec<Case> {
    let suite: contract::Suite = serde_json::from_str(include_str!(
        "../../../ara-dev/src/lab/stress/cases.v1.json"
    ))
    .expect("T1's catalogue parses");
    let mut out: Vec<Case> = suite
        .cases
        .into_iter()
        .filter(|c| c.gates.iter().any(|g| *g == 2 || *g == 4))
        .filter(|c| RECIPES.contains(&c.recipe.as_str()))
        .map(|c| Case {
            id: c.id.clone(),
            gates: c.gates.clone(),
            recipe: c.recipe.clone(),
            frames: c.frames,
            event_frame: c.event_frame,
            bodies: c.bodies,
            triangles: c.triangles,
            lights: c.lights,
            range_m: c.range_m,
            sun_deg_s: c.sun_deg_s,
            elevation_deg: c.elevation_deg,
            contact_m: c.contact_m,
            catalogue: Some(c),
        })
        .collect();
    // B2's scale stress: 2,000 buildings 4-30 m tall over 400 m, a street-level camera,
    // the sun at the worst band and one higher.
    for (speed, el) in [(1_f32, 10_f32), (10., 10.), (1., 30.)] {
        out.push(Case {
            id: format!("city-{speed}deg-{el}elev"),
            gates: vec![2],
            recipe: "city".into(),
            frames: 308,
            event_frame: 8,
            bodies: 0,
            triangles: 108,
            lights: 0,
            range_m: 8.,
            sun_deg_s: speed,
            elevation_deg: el,
            contact_m: 0.3,
            catalogue: None,
        });
    }
    out
}

/// A fixture: the native scene, its camera and authored EV100.
pub(crate) struct Fixture {
    pub scene: Scene,
    pub camera: CameraSpec,
    pub ev100: f32,
}

/// T1's cube of `triangles` (12, 108 or 1,000) unit triangles per instance.
pub(crate) fn cube(triangles: u32) -> Geometry {
    let grids = match triangles {
        108 => [(3, 3); 6],
        1000 => [(9, 9), (9, 9), (9, 9), (9, 9), (8, 11), (8, 11)],
        _ => [(1, 1); 6],
    };
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (face, (nx, ny)) in grids.into_iter().enumerate() {
        let (normal, u, v) = match face {
            0 => (Vec3::X, -Vec3::Z, Vec3::Y),
            1 => (-Vec3::X, Vec3::Z, Vec3::Y),
            2 => (Vec3::Y, Vec3::X, -Vec3::Z),
            3 => (-Vec3::Y, Vec3::X, Vec3::Z),
            4 => (Vec3::Z, Vec3::X, Vec3::Y),
            _ => (-Vec3::Z, -Vec3::X, Vec3::Y),
        };
        let start = vertices.len() as u32;
        for y in 0..=ny {
            for x in 0..=nx {
                let uv = [x as f32 / nx as f32, y as f32 / ny as f32];
                vertices.push(Vertex {
                    position: (normal * 0.5 + u * (uv[0] - 0.5) + v * (uv[1] - 0.5)).into(),
                    normal: normal.into(),
                    tangent: [u.x, u.y, u.z, 1.],
                    uv,
                    uv1: uv,
                    terrain_weights: 0,
                    color: u32::MAX,
                });
            }
        }
        for y in 0..ny {
            for x in 0..nx {
                let a = start + y * (nx + 1) + x;
                let (b, c) = (a + 1, a + nx + 1);
                indices.extend([a, b, c + 1, a, c + 1, c]);
            }
        }
    }
    let mut key = [0; 32];
    key[..4].copy_from_slice(&triangles.to_le_bytes());
    key[4] = 3;
    Geometry {
        key,
        vertices,
        lods: vec![Lod { indices, error: 0. }],
        bounds: [Vec3::splat(-0.5), Vec3::splat(0.5)],
        deforming: false,
        vertex_colors: false,
        revision: 0,
    }
}

fn material(color: [f32; 3], roughness: f32) -> Material {
    let mut m = Material::default();
    m.pbr.textures = [u32::MAX; 4];
    m.pbr.base_color = [color[0], color[1], color[2], 1.];
    m.pbr.emissive_roughness[3] = roughness;
    m
}

fn add(scene: &mut Scene, id: u64, geometry: usize, material: usize, position: Vec3, scale: Vec3) {
    let world = Mat4::from_scale_rotation_translation(scale, Quat::IDENTITY, position);
    scene.instances.push(Instance {
        entity: id,
        name: format!("fixture-{id}"),
        geometry,
        material,
        world,
        previous_world: world,
        casts_shadows: true,
        paint: None,
        receives_decals: true,
    });
}

fn body_position(i: u32, t: f32) -> Vec3 {
    let x = (i % 40) as f32 * 0.35 - 7.;
    let z = ((i / 40) % 25) as f32 * 0.35 - 4.;
    let y = (i / 1000) as f32 * 0.35 + 0.3;
    Vec3::new(
        x + (t * 0.8 + i as f32 * 0.03).sin() * t.min(1.),
        y + (z + 4.) * 0.45 + (t * 3. + i as f32).sin().abs() * 0.2,
        z - t * 1.2,
    )
}

/// Collapse bodies' pose at event time `t` (T1's `body_position`, collapse branch).
fn collapse_position(i: u32, t: f32) -> Vec3 {
    let z = ((i / 40) % 25) as f32 * 0.35 - 4.;
    let row = ((i / 10) % 30) as f32 * 0.32;
    Vec3::new(
        (i % 10) as f32 * 0.4 - 2. + t * 0.3,
        row + 0.2 - (t * t * 2.).min(row),
        z * 0.2,
    )
}

/// T1's key-light power: scaled with the authored range.
fn light_power(case: &Case) -> f32 {
    180. * case.range_m * case.range_m
}

/// The case's scene at frame zero (T1's `fixture::create` for these recipes).
pub(crate) fn create(case: &Case) -> Fixture {
    let recipe = case.recipe.as_str();
    let mut scene = Scene {
        geometry: vec![cube(12), cube(case.triangles)],
        instances: Vec::new(),
        materials: vec![
            material([0.7; 3], 0.7),
            material([0.7, 0.12, 0.05], 0.7),
            material([0.94; 3], 0.7),
        ],
        textures: 0,
        decals: Vec::new(),
        lights: Vec::new(),
        sun_direction: Vec3::new(0.4, 0.8, 0.3).normalize(),
        sun_lux: Vec3::splat(200.),
        environment: vec![[0.1, 0.12, 0.16, 1.]],
        environment_size: [1, 1],
    };
    let floor = if recipe == "flythrough" {
        600.
    } else {
        12_f32.max(20. + case.contact_m * 4.)
    };
    add(
        &mut scene,
        1,
        0,
        0,
        Vec3::new(0., -0.1, 0.),
        Vec3::new(floor, 0.2, floor),
    );
    add(
        &mut scene,
        2,
        0,
        2,
        Vec3::new(0., 2., -5.),
        Vec3::new(12., 4., 0.2),
    );
    add(
        &mut scene,
        3,
        0,
        0,
        Vec3::new(-6., 2., 0.),
        Vec3::new(0.2, 4., 10.),
    );
    if case.lights == 1 {
        // T1's key light, outside the breach wall and the body field.
        scene.lights.push(Light {
            position_range: [0., 4., 5., case.range_m],
            color_lumens: [1., 0.85, 0.65, light_power(case)],
            direction_outer: [0., -1., 0., -1.],
            parameters: [0., 1., 0.1, 1.],
            ..Light::ZERO
        });
    }
    match recipe {
        "wall" => add(
            &mut scene,
            10,
            0,
            1,
            Vec3::new(0., 1.5, 0.),
            Vec3::new(2., 3., 0.15),
        ),
        "overlap" | "eight_lights" => {
            for (i, p) in [Vec3::new(-0.2, 0.6, 0.), Vec3::new(0.2, 1.4, -0.4)]
                .into_iter()
                .enumerate()
            {
                add(&mut scene, 10 + i as u64, 0, 1, p, Vec3::new(0.3, 0.8, 2.));
            }
        }
        "crate_gap" => {
            for i in 0..2 {
                add(
                    &mut scene,
                    10 + i,
                    0,
                    1,
                    Vec3::new(i as f32 * 1.02 - 0.51, 0.5, 0.),
                    Vec3::ONE,
                );
            }
        }
        "hand" => {
            add(
                &mut scene,
                10,
                0,
                1,
                Vec3::new(0., 0.12, 0.),
                Vec3::new(0.8, 0.15, 0.65),
            );
            for i in 0..5 {
                add(
                    &mut scene,
                    11 + i,
                    0,
                    1,
                    Vec3::new(i as f32 * 0.18 - 0.36, 0.13, -0.55),
                    Vec3::new(0.12, 0.12, 0.65),
                );
            }
        }
        "stained_glass" => {
            for (i, color) in [[0.9, 0.1, 0.1], [0.1, 0.9, 0.2], [0.1, 0.2, 0.9]]
                .into_iter()
                .enumerate()
            {
                let mut m = material(color, 0.03);
                m.pbr.flags[0] = 2;
                m.pbr.base_color[3] = 0.3;
                m.extras.transmission_factor = 0.9;
                scene.materials.push(m);
                let slot = scene.materials.len() - 1;
                add(
                    &mut scene,
                    10 + i as u64,
                    0,
                    slot,
                    Vec3::new(i as f32 * 0.3 - 0.3, 0.7 + i as f32 * 0.4, 0.),
                    Vec3::new(1.5, 0.03, 1.5),
                );
            }
        }
        "contact_sun" | "contact_rect" | "rect_orientation" => add(
            &mut scene,
            10,
            0,
            1,
            Vec3::new(0., case.contact_m + 0.025, 0.),
            Vec3::new(2., 0.05, 1.),
        ),
        _ => {}
    }
    if matches!(recipe, "sun" | "pole_sweep") {
        for i in 0..25 {
            add(
                &mut scene,
                1000 + i,
                1,
                1,
                Vec3::new((i % 5) as f32 * 1.5 - 3., 1., (i / 5) as f32 * 1.5 - 3.),
                Vec3::new(0.12, 2., 0.12),
            );
        }
    }
    if recipe == "flythrough" {
        for i in 0..800 {
            add(
                &mut scene,
                1000 + i,
                1,
                1,
                Vec3::new(
                    if i % 2 == 0 { -5. } else { 5. },
                    2.,
                    150. - (i / 2) as f32 * 1.5,
                ),
                Vec3::new(1., 4., 1.),
            );
        }
    }
    if recipe == "whip_pan" {
        for i in 0..64 {
            let height = 0.5 + (i % 3) as f32 * 0.5;
            add(
                &mut scene,
                1000 + i,
                1,
                1,
                Vec3::new(
                    (i % 8) as f32 * 1.3 - 4.55,
                    height * 0.5,
                    (i / 8) as f32 - 3.5,
                ),
                Vec3::new(0.5, height, 0.5),
            );
        }
    }
    if recipe == "city" {
        // A deterministic block field: 40 x 50 lots of 10 m, buildings 6-9 m wide and 4-30 m
        // tall by a fixed hash, the street x in [0, 10) kept clear.
        add(
            &mut scene,
            4,
            0,
            0,
            Vec3::new(0., -0.1, 0.),
            Vec3::new(800., 0.2, 800.),
        );
        let mut id = 20_000;
        for gz in -10_i32..40 {
            for gx in -20_i32..20 {
                if gx == 0 {
                    continue;
                }
                let hash = (gx.wrapping_mul(73_856_093) ^ gz.wrapping_mul(19_349_663)) as u32;
                let height = 4. + (hash % 27) as f32;
                let width = 6. + (hash / 27 % 4) as f32;
                let centre = Vec3::new(gx as f32 * 10. + 5., height * 0.5, gz as f32 * 10. + 5.);
                add(
                    &mut scene,
                    id,
                    1,
                    0,
                    centre,
                    Vec3::new(width, height, width),
                );
                id += 1;
            }
        }
    }
    for i in 0..case.bodies {
        let wind = recipe == "wind";
        let position = if recipe == "collapse" {
            collapse_position(i, 0.)
        } else {
            body_position(i, 0.)
        };
        add(
            &mut scene,
            100 + u64::from(i),
            1,
            usize::from(wind),
            position,
            if wind {
                Vec3::new(0.3, 1.5, 0.03)
            } else {
                Vec3::splat(0.25)
            },
        );
    }
    if matches!(
        recipe,
        "overlap" | "crate_gap" | "hand" | "stained_glass" | "rect_orientation" | "contact_rect"
    ) {
        scene.sun_lux = Vec3::ZERO;
        let rotated = recipe == "rect_orientation";
        scene.lights = vec![Light {
            position_range: [-1., 4_f32.max(case.contact_m + 4.), 1., case.range_m],
            color_lumens: [1., 1., 1., 10000.],
            direction_outer: [0., -1., 0., -1.],
            parameters: [2., 1., 0., 1.],
            tangent_length: [
                if rotated { 0. } else { 1. },
                0.,
                if rotated { 1. } else { 0. },
                0.,
            ],
            rect: [0.6, 0.2, 0., 0.],
            ..Light::ZERO
        }];
    }
    if recipe == "contact_sun" {
        scene.lights.clear();
    }
    if recipe == "eight_lights" {
        scene.sun_lux = Vec3::ZERO;
        scene.lights = (0..8)
            .map(|i| Light {
                position_range: [
                    (i % 4) as f32 * 0.25 - 0.375,
                    4.,
                    (i / 4) as f32 * 0.3 - 0.15,
                    case.range_m,
                ],
                color_lumens: [1., 1., 1., 1250.],
                parameters: [0., 1., 0.03, 1.],
                direction_outer: [0., -1., 0., -1.],
                ..Light::ZERO
            })
            .collect();
    }
    let camera = if recipe == "city" {
        CameraSpec::LookAt {
            position: Vec3::new(5., 1.8, 2.),
            target: Vec3::new(5., 4., 60.),
            vertical_fov: 55_f32.to_radians(),
        }
    } else if case.gates.contains(&4) {
        CameraSpec::LookAt {
            position: Vec3::new(2.7, 3.0, 3.6) + Vec3::splat((case.contact_m - 0.3).max(0.)),
            target: Vec3::new(-case.contact_m * 0.5, 0.25, 0.),
            vertical_fov: 45_f32.to_radians(),
        }
    } else {
        CameraSpec::LookAt {
            position: Vec3::new(8., 6., 10.),
            target: Vec3::new(0., 0.7, 0.),
            vertical_fov: 55_f32.to_radians(),
        }
    };
    let mut f = Fixture {
        scene,
        camera,
        ev100: 5.,
    };
    apply(&mut f, case, 0);
    f
}

/// The absolute scripted pose of `frame` (60 Hz; T1's `fixture::apply` and `camera`).
/// The wall breach replaces the wall with 24 fragments at the event frame (a layout
/// change: the caller republishes the scene).
pub(crate) fn apply(f: &mut Fixture, case: &Case, frame: u32) {
    let recipe = case.recipe.as_str();
    let seconds = frame as f32 / 60.;
    let t = if frame >= case.event_frame {
        (frame - case.event_frame) as f32 / 60.
    } else {
        0.
    };
    if frame == case.event_frame && recipe == "wall" {
        f.scene.instances.retain(|i| i.entity != 10);
        for i in 0..24 {
            add(
                &mut f.scene,
                10_000 + i,
                0,
                1,
                Vec3::new((i % 6) as f32 * 0.35 - 0.9, (i / 6) as f32 * 0.6 + 0.3, 0.),
                Vec3::new(0.3, 0.5, 0.15),
            );
        }
    }
    for instance in &mut f.scene.instances {
        instance.previous_world = instance.world;
        if (100..100 + u64::from(case.bodies)).contains(&instance.entity) {
            let i = (instance.entity - 100) as u32;
            if recipe == "wind" {
                instance.world = Mat4::from_scale_rotation_translation(
                    Vec3::new(0.3, 1.5, 0.03),
                    Quat::from_rotation_z((t * 2. + i as f32).sin() * 0.15),
                    body_position(i, 0.),
                );
            } else if recipe == "collapse" {
                instance.world.w_axis = collapse_position(i, t).extend(1.);
            } else {
                instance.world.w_axis = body_position(i, t).extend(1.);
            }
        }
        if instance.entity >= 10_000 && instance.entity < 20_000 {
            let i = instance.entity - 10_000;
            instance.world.w_axis = Vec3::new(
                (i % 6) as f32 * 0.35 - 0.9 + t * (i as f32 * 0.1 - 1.),
                (0.3 + (i / 6) as f32 * 0.6 - 4.9 * t * t).max(0.15),
                t * 0.4,
            )
            .extend(1.);
        }
    }
    let az = (seconds * case.sun_deg_s).to_radians();
    let el = case.elevation_deg.to_radians();
    f.scene.sun_direction = Vec3::new(el.cos() * az.cos(), el.sin(), el.cos() * az.sin());
    if recipe == "whip_pan" {
        let a = (t / 0.5).min(1.) * std::f32::consts::PI;
        f.camera = CameraSpec::LookAt {
            position: Vec3::new(0., 2., 3.),
            target: Vec3::new(a.sin() * 5., 1., 3. - a.cos() * 5.),
            vertical_fov: 55_f32.to_radians(),
        };
    }
    if recipe == "flythrough" {
        let z = 150. - t * (300. / 3.6);
        f.camera = CameraSpec::LookAt {
            position: Vec3::new(0., 3., z),
            target: Vec3::new(0., 1., z - 10.),
            vertical_fov: 55_f32.to_radians(),
        };
    }
}

/// The fixture as a world-space extraction for the path-traced reference
/// (`ara-render-fidelity`'s `FidelityView::new` reads the legacy scene).
pub(crate) fn legacy(f: &Fixture) -> ara_render::scene::Scene {
    use ara_render::scene;
    let mut meshes = Vec::new();
    for instance in &f.scene.instances {
        let g = &f.scene.geometry[instance.geometry];
        let normal = Mat4::from_mat3(glam::Mat3::from_mat4(instance.world).inverse().transpose());
        meshes.push(scene::Mesh {
            entity: instance.entity,
            name: instance.name.clone(),
            vertices: g
                .vertices
                .iter()
                .map(|v| {
                    let n = normal.transform_vector3(Vec3::from(v.normal)).normalize();
                    let t = instance
                        .world
                        .transform_vector3(Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]))
                        .normalize();
                    scene::Vertex {
                        position: instance
                            .world
                            .transform_point3(Vec3::from(v.position))
                            .into(),
                        normal: n.into(),
                        uv: v.uv,
                        tangent: [t.x, t.y, t.z, v.tangent[3]],
                    }
                })
                .collect(),
            indices: g.lods[0].indices.clone(),
            material: instance.material,
        });
    }
    let materials = f
        .scene
        .materials
        .iter()
        .map(|m| {
            let mut p = m.pbr;
            p.textures = [0, 1, 0, 0];
            p
        })
        .collect();
    scene::Scene {
        meshes,
        materials,
        textures: vec![
            scene::Texture {
                size: [1, 1],
                rgba: vec![255; 4],
                srgb: true,
            },
            scene::Texture {
                size: [1, 1],
                rgba: vec![128, 128, 255, 255],
                srgb: false,
            },
        ],
        lights: f.scene.lights.clone(),
        sun_direction: f.scene.sun_direction,
        sun_lux: f.scene.sun_lux,
        environment: f.scene.environment.clone(),
        environment_size: f.scene.environment_size,
    }
}

/// Direct sun (or the rect light) only: a black environment, so images compare shadows
/// rather than ambient models.
pub(crate) fn direct_only(f: &mut Fixture) {
    f.scene.environment = vec![[0., 0., 0., 1.]];
}
