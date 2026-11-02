//! The G2/G4 shadow fixtures, rebuilt from T1's lab recipes (`ara-dev` lab
//! `stress/cases.v1.json` and `render-fidelity` example `render_v3/fixture.rs` at
//! fb3f8ad77; T1's tooling is not merged into this branch): the same geometry, motion,
//! sun script and camera for the recipes the sun page service serves. Rect-light R5-5
//! recipes (overlap, crate gap, hand, stained glass, rect orientation, contact rect,
//! eight lights) shade through local lights, not sun pages, and are not built here.
use ara_render::instanced::{Geometry, Instance, Lod, Material, Scene, Vertex};
use ara_render::scene::CameraSpec;
use glam::{Mat4, Quat, Vec3};

/// One case of the catalogue (fields as `cases.v1.json`).
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Case {
    pub id: String,
    pub gates: Vec<u8>,
    pub recipe: &'static str,
    pub frames: u32,
    pub event_frame: u32,
    pub bodies: u32,
    pub triangles: u32,
    pub sun_deg_s: f32,
    pub elevation_deg: f32,
    pub contact_m: f32,
}

/// The G2 and G4 cases of the sun page service, in catalogue order.
pub(crate) fn cases() -> Vec<Case> {
    let case =
        |id: String, gates: Vec<u8>, recipe, frames, bodies, triangles, sun, el, contact| Case {
            id,
            gates,
            recipe,
            frames,
            event_frame: if frames > 32 { 8 } else { 0 },
            bodies,
            triangles,
            sun_deg_s: sun,
            elevation_deg: el,
            contact_m: contact,
        };
    let mut out = Vec::new();
    for speed in [0.25_f32, 1., 10.] {
        for el in [60_f32, 30., 10., 5.] {
            let name = if speed < 1. {
                "0.25".to_string()
            } else {
                format!("{speed}")
            };
            out.push(case(
                format!("sun-{name}deg-{el}elev"),
                vec![2],
                "sun",
                308,
                0,
                108,
                speed,
                el,
                0.3,
            ));
        }
    }
    out.push(case(
        "foliage-wind".into(),
        vec![2],
        "wind",
        308,
        1200,
        108,
        0.,
        45.,
        0.3,
    ));
    out.push(case(
        "avalanche-108".into(),
        vec![2, 3],
        "avalanche",
        308,
        4000,
        108,
        0.,
        45.,
        0.3,
    ));
    out.push(case(
        "avalanche-1000".into(),
        vec![2, 3],
        "avalanche",
        308,
        4000,
        1000,
        0.,
        45.,
        0.3,
    ));
    for h in [0.1_f32, 0.3, 1., 3., 10.] {
        out.push(case(
            format!("shadow-contact-sun-{h}"),
            vec![4],
            "contact_sun",
            32,
            0,
            108,
            0.,
            45.,
            h,
        ));
    }
    out.push(case(
        "shadow-pole-sweep".into(),
        vec![4],
        "pole_sweep",
        32,
        0,
        108,
        10.,
        45.,
        0.3,
    ));
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

/// The case's scene at frame zero (T1's `fixture::create` for these recipes).
pub(crate) fn create(case: &Case) -> Fixture {
    let mut scene = Scene {
        geometry: vec![cube(12), cube(case.triangles)],
        instances: Vec::new(),
        materials: vec![material([0.7; 3], 0.7), material([0.7, 0.12, 0.05], 0.7)],
        textures: 0,
        decals: Vec::new(),
        lights: Vec::new(),
        sun_direction: Vec3::new(0.4, 0.8, 0.3).normalize(),
        sun_lux: Vec3::splat(200.),
        environment: vec![[0.1, 0.12, 0.16, 1.]],
        environment_size: [1, 1],
    };
    let floor = 12_f32.max(20. + case.contact_m * 4.);
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
        0,
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
    if case.recipe == "contact_sun" {
        add(
            &mut scene,
            10,
            0,
            1,
            Vec3::new(0., case.contact_m + 0.025, 0.),
            Vec3::new(2., 0.05, 1.),
        );
    }
    if matches!(case.recipe, "sun" | "pole_sweep") {
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
    for i in 0..case.bodies {
        let wind = case.recipe == "wind";
        add(
            &mut scene,
            100 + u64::from(i),
            1,
            usize::from(wind),
            body_position(i, 0.),
            if wind {
                Vec3::new(0.3, 1.5, 0.03)
            } else {
                Vec3::splat(0.25)
            },
        );
    }
    let camera = if case.gates.contains(&4) {
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

/// The absolute scripted pose of `frame` (60 Hz; T1's `fixture::apply`).
pub(crate) fn apply(f: &mut Fixture, case: &Case, frame: u32) {
    let seconds = frame as f32 / 60.;
    let t = frame.saturating_sub(case.event_frame) as f32 / 60.;
    let t = if frame >= case.event_frame { t } else { 0. };
    for instance in &mut f.scene.instances {
        instance.previous_world = instance.world;
        if (100..100 + u64::from(case.bodies)).contains(&instance.entity) {
            let i = (instance.entity - 100) as u32;
            if case.recipe == "wind" {
                instance.world = Mat4::from_scale_rotation_translation(
                    Vec3::new(0.3, 1.5, 0.03),
                    Quat::from_rotation_z((t * 2. + i as f32).sin() * 0.15),
                    body_position(i, 0.),
                );
            } else {
                instance.world.w_axis = body_position(i, t).extend(1.);
            }
        }
    }
    let az = (seconds * case.sun_deg_s).to_radians();
    let el = case.elevation_deg.to_radians();
    f.scene.sun_direction = Vec3::new(el.cos() * az.cos(), el.sin(), el.cos() * az.sin());
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
        lights: Vec::new(),
        sun_direction: f.scene.sun_direction,
        sun_lux: f.scene.sun_lux,
        environment: f.scene.environment.clone(),
        environment_size: f.scene.environment_size,
    }
}
