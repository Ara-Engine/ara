//! Deterministic fallback recipes with the T1 G3 event counts and durations.
use super::fixture_api::{
    instanced::{self, transport::BodyClass},
    scene::{self, CameraSpec, Light},
};
use anyhow::Result;
use glam::{Mat4, Vec3};
use sha2::Digest;
use std::collections::BTreeMap;

pub struct Case {
    pub name: &'static str,
    pub frames: u64,
    pub event: u64,
    pub bodies: u32,
    pub triangles: u32,
    pub lights: u32,
}
pub struct Fixture {
    pub scene: instanced::Scene,
    pub textures: Vec<scene::Texture>,
    pub bodies: BTreeMap<u64, BodyClass>,
    pub camera: CameraSpec,
    pub exposure: f32,
}
pub fn cases() -> Vec<Case> {
    [
        ("door", 90, 0, 108, 1),
        ("recolour", 90, 0, 108, 1),
        ("switch-off", 90, 0, 108, 1),
        ("200-light", 308, 0, 108, 200),
        ("sealed-flicker", 90, 0, 108, 200),
        ("sealed-20", 90, 0, 108, 20),
        ("open-flicker", 90, 0, 108, 200),
        ("repeated-invalidation", 308, 0, 108, 1),
        ("wall", 90, 0, 108, 1),
        ("collapse", 90, 300, 108, 1),
        ("avalanche-108", 308, 4000, 108, 1),
        ("avalanche-1000", 308, 4000, 1000, 1),
        ("camera-cut", 90, 0, 108, 1),
        ("sleep-wake", 90, 64, 108, 1),
    ]
    .into_iter()
    .map(|(name, frames, bodies, triangles, lights)| Case {
        name,
        frames,
        event: 8,
        bodies,
        triangles,
        lights,
    })
    .collect()
}
pub fn cube(triangles: u32) -> instanced::Geometry {
    let divisions = if triangles >= 972 {
        9
    } else if triangles >= 108 {
        3
    } else {
        1
    };
    let mut faces = Vec::new();
    for n in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        let u = if n.x.abs() > 0.5 { Vec3::Y } else { Vec3::X };
        let v = n.cross(u);
        for y in 0..divisions {
            for x in 0..divisions {
                let p = |x: u32, y: u32| {
                    n * 0.5
                        + u * (x as f32 / divisions as f32 - 0.5)
                        + v * (y as f32 / divisions as f32 - 0.5)
                };
                faces.push(([p(x, y), p(x + 1, y), p(x + 1, y + 1)], n));
                faces.push(([p(x, y), p(x + 1, y + 1), p(x, y + 1)], n));
            }
        }
    }
    while (faces.len() as u32) < triangles {
        let (p, n) = faces.remove(0);
        let c = (p[0] + p[1] + p[2]) / 3.;
        for i in 0..3 {
            faces.push(([p[i], p[(i + 1) % 3], c], n));
        }
    }
    let vertices: Vec<_> = faces
        .into_iter()
        .flat_map(|(p, n)| {
            p.map(|p| instanced::Vertex {
                position: p.to_array(),
                normal: n.to_array(),
                uv: [0.; 2],
                tangent: [1., 0., 0., 1.],
                uv1: [0.; 2],
                terrain_weights: 0,
                color: instanced::Vertex::WHITE,
            })
        })
        .collect();
    let key = sha2::Sha256::digest(bytemuck::cast_slice(&vertices)).into();
    let indices = (0..vertices.len() as u32).collect();
    instanced::Geometry {
        key,
        vertices,
        lods: vec![instanced::Lod { indices, error: 0. }],
        bounds: [Vec3::splat(-0.5), Vec3::splat(0.5)],
        deforming: false,
        vertex_colors: false,
        revision: 0,
    }
}
fn add(
    scene: &mut instanced::Scene,
    entity: u64,
    geometry: usize,
    material: usize,
    pos: Vec3,
    scale: Vec3,
) {
    let world = Mat4::from_scale_rotation_translation(scale, glam::Quat::IDENTITY, pos);
    scene.instances.push(instanced::Instance {
        entity,
        name: format!("G3-{entity}"),
        geometry,
        material,
        world,
        previous_world: world,
        casts_shadows: true,
        paint: None,
        receives_decals: true,
    });
}
pub fn create(case: &Case) -> Result<Fixture> {
    let materials = [[0.7, 0.7, 0.7], [0.7, 0.12, 0.05], [0.94, 0.94, 0.94]].map(|rgb| {
        let mut m = instanced::Material::default();
        m.pbr.base_color = [rgb[0], rgb[1], rgb[2], 1.];
        m
    });
    let mut scene = instanced::Scene {
        geometry: vec![cube(12), cube(case.triangles)],
        instances: Vec::new(),
        materials: materials.into(),
        textures: 0,
        decals: Vec::new(),
        lights: Vec::new(),
        sun_direction: Vec3::new(0.4, 0.8, 0.3).normalize(),
        sun_lux: Vec3::splat(200.),
        environment: vec![[0.1, 0.12, 0.16, 1.]],
        environment_size: [1, 1],
    };
    add(
        &mut scene,
        1,
        0,
        0,
        Vec3::new(0., -0.1, 0.),
        Vec3::new(20., 0.2, 20.),
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
    if matches!(
        case.name,
        "door" | "recolour" | "switch-off" | "repeated-invalidation" | "wall"
    ) {
        add(
            &mut scene,
            10,
            0,
            1,
            Vec3::new(0., 1.5, 0.),
            Vec3::new(2., 3., 0.15),
        );
    }
    if matches!(case.name, "sealed-flicker" | "sealed-20") {
        for i in 0..=20 {
            add(
                &mut scene,
                5000 + i,
                0,
                0,
                Vec3::new(i as f32 * 0.6 - 6., 2., 0.),
                Vec3::new(0.04, 4., 12.),
            );
        }
        for i in 0..=10 {
            add(
                &mut scene,
                5100 + i,
                0,
                0,
                Vec3::new(0., 2., i as f32 * 1.2 - 6.),
                Vec3::new(12., 4., 0.04),
            );
        }
        add(
            &mut scene,
            5200,
            0,
            0,
            Vec3::new(0., 4., 0.),
            Vec3::new(12., 0.1, 12.),
        );
    }
    let mut bodies = BTreeMap::new();
    for i in 0..case.bodies {
        let row = scene.instances.len() as u64;
        add(
            &mut scene,
            100 + i as u64,
            1,
            0,
            body_position(case, i, 0.),
            Vec3::splat(0.25),
        );
        bodies.insert(row, BodyClass::PhysicsActive);
    }
    for i in 0..case.lights {
        scene.lights.push(Light {
            position_range: if case.lights == 1 {
                [0., 4., 5., 8.]
            } else {
                [
                    (i % 20) as f32 * 0.6 - 5.7,
                    2.4,
                    (i / 20) as f32 * 1.2 - 5.4,
                    8.,
                ]
            },
            color_lumens: [1., 0.85, 0.65, 11520.],
            direction_outer: [0., -1., 0., -1.],
            parameters: [0., 1., 0.1, 1.],
            ..Light::ZERO
        });
    }
    if matches!(
        case.name,
        "switch-off" | "sealed-flicker" | "sealed-20" | "open-flicker" | "repeated-invalidation"
    ) {
        scene.sun_lux = Vec3::ZERO;
        scene.environment = vec![[0., 0., 0., 1.]];
    }
    scene.validate()?;
    Ok(Fixture {
        scene,
        textures: Vec::new(),
        bodies,
        camera: CameraSpec::LookAt {
            position: Vec3::new(8., 6., 10.),
            target: Vec3::new(0., 0.7, 0.),
            vertical_fov: 55f32.to_radians(),
        },
        exposure: 1. / (1.2 * 32.),
    })
}
fn body_position(case: &Case, i: u32, t: f32) -> Vec3 {
    if case.name == "collapse" {
        Vec3::new(
            (i % 10) as f32 * 0.4 - 2. + t * 0.3,
            (((i / 10) % 30) as f32 * 0.32 - 2. * t * t).max(0.) + 0.2,
            (i / 300) as f32 * 0.35,
        )
    } else {
        Vec3::new(
            (i % 40) as f32 * 0.35 - 7. + (t * 0.8 + i as f32 * 0.03).sin() * t.min(1.),
            (i / 1000) as f32 * 0.35
                + 0.3
                + ((i / 40) % 25) as f32 * 0.35 * 0.45
                + (t * 3. + i as f32).sin().abs() * 0.2,
            ((i / 40) % 25) as f32 * 0.35 - 4. - t * 1.2,
        )
    }
}
pub fn advance(f: &mut Fixture, case: &Case, frame: u64) -> instanced::Changes {
    let mut changes = instanced::Changes::default();
    if frame < case.event {
        return changes;
    }
    let t = (frame - case.event) as f32 / 60.;
    if frame == case.event && matches!(case.name, "wall" | "camera-cut") {
        if case.name == "wall" {
            f.scene.instances.retain(|i| i.entity != 10);
        }
        let slot = if case.name == "camera-cut" {
            let slot = f.scene.geometry.len();
            f.scene.geometry.push(cube(1000));
            changes.geometries.push(slot);
            slot
        } else {
            0
        };
        for i in 0..24 {
            let row = f.scene.instances.len();
            add(
                &mut f.scene,
                10_000 + i,
                slot,
                1,
                Vec3::new((i % 6) as f32 * 0.35 - 0.9, (i / 6) as f32 * 0.6 + 0.3, 0.),
                if case.name == "wall" {
                    Vec3::new(0.3, 0.5, 0.15)
                } else {
                    Vec3::splat(0.25)
                },
            );
            f.bodies.insert(row as u64, BodyClass::PhysicsActive);
            changes
                .transport
                .bodies
                .push((row, BodyClass::PhysicsActive));
            changes.instances.push(row);
        }
    }
    for (row, instance) in f.scene.instances.iter_mut().enumerate() {
        instance.previous_world = instance.world;
        if instance.entity == 10 && case.name == "door" {
            instance.world = Mat4::from_translation(Vec3::new(-1., 0., 0.))
                * Mat4::from_rotation_y((t * 3.).min(std::f32::consts::FRAC_PI_2))
                * Mat4::from_translation(Vec3::new(1., 1.5, 0.))
                * Mat4::from_scale(Vec3::new(2., 3., 0.15));
        }
        if instance.entity >= 10_000 {
            let i = instance.entity - 10_000;
            instance.world.w_axis = Vec3::new(
                (i % 6) as f32 * 0.35 - 0.9 + t * (i as f32 * 0.1 - 1.),
                (0.3 + (i / 6) as f32 * 0.6 - 4.9 * t * t).max(0.15),
                t * 0.4,
            )
            .extend(1.);
        }
        if (100..100 + case.bodies as u64).contains(&instance.entity) {
            let sleeping = case.name == "sleep-wake" && (28..=55).contains(&frame);
            instance.world.w_axis = body_position(
                case,
                (instance.entity - 100) as u32,
                if sleeping { 20. / 60. } else { t },
            )
            .extend(1.);
            let class = if sleeping {
                BodyClass::Sleeping
            } else {
                BodyClass::PhysicsActive
            };
            if f.bodies.insert(row as u64, class) != Some(class) {
                changes.transport.bodies.push((row, class));
            }
        }
        if instance.previous_world != instance.world {
            changes.instances.push(row);
        }
    }
    if frame == case.event && case.name == "recolour" {
        f.scene.materials[1].pbr.base_color = [0.05, 0.15, 0.85, 1.];
        changes.materials.push(1);
    }
    for (i, light) in f.scene.lights.iter_mut().enumerate() {
        // F8 is the first changed sample, after the unchanged pre-event frames.
        let light_time = t + 1. / 60.;
        if case.name == "switch-off" {
            light.color_lumens[3] = 0.;
        }
        if matches!(case.name, "sealed-flicker" | "sealed-20" | "open-flicker") {
            light.color_lumens[3] = if (frame + i as u64) % 7 < 3 {
                11520.
            } else {
                0.
            };
        }
        if case.name == "200-light" {
            light.position_range[0] = (i % 20) as f32 * 0.6 - 5.7
                + ((light_time * 1.7 + i as f32).sin() - (i as f32).sin()) * 1.6;
        }
        if case.name == "repeated-invalidation" {
            light.position_range[0] = (light_time * 2.).sin();
            light.color_lumens[3] = if frame.is_multiple_of(2) { 11520. } else { 0. };
        }
    }
    if frame == case.event && case.name == "camera-cut" {
        f.camera = CameraSpec::LookAt {
            position: Vec3::new(-4., 3., -4.),
            target: Vec3::ZERO,
            vertical_fov: 55f32.to_radians(),
        };
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_lights_change_on_all_300_event_frames() -> Result<()> {
        for case in cases()
            .into_iter()
            .filter(|case| matches!(case.name, "200-light" | "repeated-invalidation"))
        {
            let mut fixture = create(&case)?;
            let mut changed = 0;
            for frame in 0..case.frames {
                let before: Vec<_> = fixture
                    .scene
                    .lights
                    .iter()
                    .map(|light| (light.position_range, light.color_lumens))
                    .collect();
                advance(&mut fixture, &case, frame);
                let differs = fixture
                    .scene
                    .lights
                    .iter()
                    .zip(before)
                    .any(|(light, old)| (light.position_range, light.color_lumens) != old);
                assert_eq!(differs, frame >= case.event, "{} F{frame}", case.name);
                changed += usize::from(differs);
            }
            assert_eq!(changed, 300, "{}", case.name);
        }
        Ok(())
    }
}
