use super::{
    contract::{Case, FrameScript},
    geometry::{add, cube, material},
};
use ara_render::{
    instanced::Scene,
    scene::{CameraSpec, Light},
};
use glam::{Mat4, Quat, Vec3};

pub struct Fixture {
    pub scene: Scene,
    pub camera: CameraSpec,
    pub extent: f32,
    pub ev100: f32,
    pub textures: Vec<([u8; 32], std::sync::Arc<ara_texture_stream::NativeSource>)>,
}

pub fn create(case: &Case) -> anyhow::Result<Fixture> {
    if case.recipe == "polar" {
        return super::polar::load();
    }
    let extent = case.target_lpp.map_or(12., |lpp| {
        (f64::from(case.lights) * std::f64::consts::PI * (f64::from(case.range_m) * 0.95).powi(2)
            / lpp)
            .sqrt() as f32
    });
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
    let floor = if case.recipe == "flythrough" {
        600.
    } else {
        extent.max(20. + case.contact_m * 4.)
    };
    add(
        &mut scene,
        1,
        0,
        0,
        Vec3::new(0., -0.1, 0.),
        Vec3::new(floor, 0.2, floor),
    );
    if !matches!(case.recipe.as_str(), "lights" | "moving_lights" | "steady") {
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
    }
    for i in 0..case.lights {
        let cols = if case.lights == 200 {
            20
        } else {
            (case.lights as f32).sqrt().ceil() as u32
        };
        let x = ((i % cols) as f32 + 0.5) / cols as f32 - 0.5;
        let z = ((i / cols) as f32 + 0.5) / case.lights.div_ceil(cols) as f32 - 0.5;
        scene.lights.push(Light {
            position_range: if case.lights == 1 {
                // Keep the key outside the breach wall and the moving body field.
                [0., 4., 5., case.range_m]
            } else {
                [
                    x * extent * std::f32::consts::SQRT_2,
                    case.range_m * 0.3,
                    z * extent / std::f32::consts::SQRT_2,
                    case.range_m,
                ]
            },
            // Scale power with the authored range so the range sweep preserves
            // receiver illuminance while changing spatial coverage and demand.
            color_lumens: [1., 0.85, 0.65, light_power(case)],
            direction_outer: [0., -1., 0., -1.],
            parameters: [0., 1., 0.1, 1.],
            ..Light::ZERO
        });
    }
    if (case.target_lpp.is_some() && case.recipe != "combined")
        || matches!(
            case.recipe.as_str(),
            "switch_off" | "sealed_flicker" | "open_flicker" | "invalidation"
        )
    {
        scene.sun_lux = Vec3::ZERO;
        scene.environment = vec![[0., 0., 0., 1.]];
    }
    match case.recipe.as_str() {
        "wall" | "door" | "recolour" | "switch_off" | "invalidation" => {
            add(
                &mut scene,
                10,
                0,
                1,
                Vec3::new(0., 1.5, 0.),
                Vec3::new(2., 3., 0.15),
            );
        }
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
        "stained_glass" | "ordered_glass" => {
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
        "contact_sun" | "contact_rect" | "rect_orientation" => {
            add(
                &mut scene,
                10,
                0,
                1,
                Vec3::new(0., case.contact_m + 0.025, 0.),
                Vec3::new(2., 0.05, 1.),
            );
        }
        "roughness" | "polar" => {
            for i in 0..16 {
                let mut m = material([0.7; 3], i as f32 / 15.);
                m.pbr.surface[0] = 1.;
                scene.materials.push(m);
                let slot = scene.materials.len() - 1;
                add(
                    &mut scene,
                    10 + i,
                    0,
                    slot,
                    Vec3::new(i as f32 * 0.5 - 4., 0.03, 0.),
                    Vec3::new(0.5, 0.06, 8.),
                );
            }
            add(
                &mut scene,
                30,
                0,
                1,
                Vec3::new(0., 1.5, -2.),
                Vec3::new(2., 3., 1.),
            );
        }
        _ => {}
    }
    if case.recipe == "sun" || case.recipe == "pole_sweep" {
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
    if case.recipe == "flythrough" {
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
    if matches!(case.recipe.as_str(), "camera_cut" | "cold_cut" | "whip_pan") {
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
    for i in 0..case.bodies {
        let pos = body_position(case, i, 0.);
        add(
            &mut scene,
            100 + i as u64,
            1,
            if case.recipe == "wind" { 1 } else { 0 },
            pos,
            if case.recipe == "wind" {
                Vec3::new(0.3, 1.5, 0.03)
            } else {
                Vec3::splat(0.25)
            },
        );
    }
    if case.target_lpp.is_some() {
        let mut glossy = material([0.7; 3], 0.08);
        glossy.pbr.surface[0] = 1.;
        let glossy_id = scene.materials.len();
        scene.materials.push(glossy);
        // Occluders and glossy receivers exercise E3 shadows/highlights as well
        // as light enumeration. The receiver domain still includes the floor.
        for i in 0..16 {
            add(
                &mut scene,
                2000 + i,
                1,
                if i % 2 == 0 { glossy_id } else { 1 },
                Vec3::new(
                    ((i % 4) as f32 - 1.5) * case.range_m * 0.4,
                    case.range_m * 0.125,
                    ((i / 4) as f32 - 1.5) * case.range_m * 0.4,
                ),
                Vec3::new(
                    case.range_m * 0.12,
                    case.range_m * 0.25,
                    case.range_m * 0.12,
                ),
            );
        }
    }
    if case.recipe == "sealed_flicker" {
        for i in 0..=20 {
            add(
                &mut scene,
                5000 + i,
                0,
                0,
                Vec3::new(
                    (i as f32 / 20. - 0.5) * extent * std::f32::consts::SQRT_2,
                    2.,
                    0.,
                ),
                Vec3::new(0.04, 4., extent / std::f32::consts::SQRT_2),
            );
        }
        for i in 0..=10 {
            add(
                &mut scene,
                5100 + i,
                0,
                0,
                Vec3::new(
                    0.,
                    2.,
                    (i as f32 / 10. - 0.5) * extent / std::f32::consts::SQRT_2,
                ),
                Vec3::new(extent * std::f32::consts::SQRT_2, 4., 0.04),
            );
        }
        add(
            &mut scene,
            5200,
            0,
            0,
            Vec3::new(0., 4., 0.),
            Vec3::new(
                extent * std::f32::consts::SQRT_2,
                0.1,
                extent / std::f32::consts::SQRT_2,
            ),
        );
    }
    if matches!(
        case.recipe.as_str(),
        "overlap" | "crate_gap" | "hand" | "stained_glass" | "rect_orientation" | "contact_rect"
    ) {
        scene.sun_lux = Vec3::ZERO;
        scene.lights = vec![Light {
            position_range: [-1., 4_f32.max(case.contact_m + 4.), 1., case.range_m],
            color_lumens: [1., 1., 1., 10000.],
            direction_outer: [0., -1., 0., -1.],
            parameters: [2., 1., 0., 1.],
            tangent_length: [1., 0., 0., 0.],
            rect: [0.6, 0.2, 0., 0.],
            ..Light::ZERO
        }];
    }
    if case.recipe == "contact_sun" {
        scene.lights.clear();
    }
    if case.recipe == "eight_lights" {
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
    let camera = if case.target_lpp.is_some() {
        CameraSpec::LookAt {
            position: Vec3::new(0., extent + 10., 0.001),
            target: Vec3::ZERO,
            vertical_fov: 2. * (extent / (4. * std::f32::consts::SQRT_2 * (extent + 10.))).atan(),
        }
    } else if case.recipe == "sealed_flicker" {
        let center = Vec3::new(
            extent * std::f32::consts::SQRT_2 / 40.,
            0.,
            extent / std::f32::consts::SQRT_2 / 20.,
        );
        CameraSpec::LookAt {
            position: center + Vec3::new(0., 0.8, 0.25),
            target: center,
            vertical_fov: 55_f32.to_radians(),
        }
    } else if case.gates.contains(&4) || case.recipe == "ordered_glass" {
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
    Ok(Fixture {
        scene,
        camera,
        extent,
        ev100: 5. + case.target_lpp.map_or(0., |lpp| (lpp / 2.4).log2() as f32),
        textures: Vec::new(),
    })
}

fn light_power(case: &Case) -> f32 {
    180. * case.range_m * case.range_m
}

fn body_position(case: &Case, i: u32, t: f32) -> Vec3 {
    let x = (i % 40) as f32 * 0.35 - 7.;
    let z = ((i / 40) % 25) as f32 * 0.35 - 4.;
    let y = (i / 1000) as f32 * 0.35 + 0.3;
    if case.recipe == "collapse" || case.recipe == "combined" {
        Vec3::new(
            (i % 10) as f32 * 0.4 - 2. + t * 0.3,
            ((i / 10) % 30) as f32 * 0.32 + 0.2 - (t * t * 2.).min(((i / 10) % 30) as f32 * 0.32),
            z * 0.2,
        )
    } else {
        Vec3::new(
            x + (t * 0.8 + i as f32 * 0.03).sin() * t.min(1.),
            y + (z + 4.) * 0.45 + (t * 3. + i as f32).sin().abs() * 0.2,
            z - t * 1.2,
        )
    }
}

/// Apply an absolute scripted pose. The initial snapshot and script define replay.
pub fn apply(f: &mut Fixture, case: &Case, frame: &FrameScript) {
    let t = frame.event_age.unwrap_or(0) as f32 / 60.;
    if frame.frame == case.event_frame && case.recipe == "wall" {
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
    if frame.cold_residency {
        let slot = f.scene.geometry.len();
        // Both cuts must publish a mesh absent from the warm content generation.
        f.scene
            .geometry
            .push(cube(if case.triangles == 108 { 1000 } else { 108 }));
        for i in 0..24 {
            add(
                &mut f.scene,
                10_000 + i,
                slot,
                1,
                Vec3::new((i % 6) as f32 * 0.35 - 0.9, 0.3, (i / 6) as f32 * 0.35),
                Vec3::splat(0.25),
            );
        }
    }
    for instance in &mut f.scene.instances {
        instance.previous_world = instance.world;
        if (100..100 + u64::from(case.bodies)).contains(&instance.entity) {
            let i = (instance.entity - 100) as u32;
            if case.recipe == "wind" {
                instance.world = Mat4::from_scale_rotation_translation(
                    Vec3::new(0.3, 1.5, 0.03),
                    Quat::from_rotation_z((t * 2. + i as f32).sin() * 0.15),
                    body_position(case, i, 0.),
                );
            } else {
                let moving = case.recipe != "sleep_wake" || !matches!(frame.frame, 28..=55);
                instance.world.w_axis =
                    body_position(case, i, if moving { t } else { 20. / 60. }).extend(1.);
            }
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
        if instance.entity == 10 && case.recipe == "door" {
            instance.world = Mat4::from_translation(Vec3::new(-1., 0., 0.))
                * Mat4::from_rotation_y((t * 3.).min(std::f32::consts::FRAC_PI_2))
                * Mat4::from_translation(Vec3::new(1., 1.5, 0.))
                * Mat4::from_scale(Vec3::new(2., 3., 0.15));
        }
    }
    if case.recipe == "recolour" && frame.event_age.is_some() {
        f.scene.materials[1].pbr.base_color = [0.05, 0.15, 0.85, 1.];
    }
    for (i, l) in f.scene.lights.iter_mut().enumerate() {
        if case.recipe == "switch_off" && frame.event_age.is_some() {
            l.color_lumens[3] = 0.;
        }
        if matches!(case.recipe.as_str(), "sealed_flicker" | "open_flicker")
            && frame.event_age.is_some()
        {
            l.color_lumens[3] = if (frame.frame + i as u32) % 7 < 3 {
                light_power(case)
            } else {
                0.
            };
        }
        if matches!(case.recipe.as_str(), "moving_lights" | "combined") {
            let cols = if case.lights == 200 {
                20
            } else {
                (case.lights as f32).sqrt().ceil() as u32
            };
            let base = ((i as u32 % cols) as f32 + 0.5) / cols as f32 - 0.5;
            l.position_range[0] = base * f.extent * std::f32::consts::SQRT_2
                + ((t * 1.7 + i as f32).sin() - (i as f32).sin()) * case.range_m * 0.2;
        }
        if case.recipe == "invalidation" && frame.event_age.is_some() {
            l.position_range[0] = (t * 2.).sin();
            l.color_lumens[3] = if frame.frame.is_multiple_of(2) {
                light_power(case)
            } else {
                0.
            };
        }
        if case.recipe == "rect_orientation" {
            l.tangent_length = [0., 0., 1., 0.];
        }
    }
    let az = (frame.seconds as f32 * case.sun_deg_s).to_radians();
    let el = case.elevation_deg.to_radians();
    f.scene.sun_direction = Vec3::new(el.cos() * az.cos(), el.sin(), el.cos() * az.sin());
    camera(f, case, frame);
}

pub fn camera(f: &mut Fixture, case: &Case, frame: &FrameScript) {
    let t = frame.event_age.unwrap_or(0) as f32 / 60.;
    if case.recipe == "whip_pan" {
        let a = (t / 0.5).min(1.) * std::f32::consts::PI;
        f.camera = CameraSpec::LookAt {
            position: Vec3::new(0., 2., 3.),
            target: Vec3::new(a.sin() * 5., 1., 3. - a.cos() * 5.),
            vertical_fov: 55_f32.to_radians(),
        };
    }
    if case.recipe == "flythrough" {
        let z = 150. - t * (300. / 3.6);
        f.camera = CameraSpec::LookAt {
            position: Vec3::new(0., 3., z),
            target: Vec3::new(0., 1., z - 10.),
            vertical_fov: 55_f32.to_radians(),
        };
    }
    if frame.camera_cut {
        f.camera = if case.target_lpp.is_some() {
            CameraSpec::LookAt {
                position: Vec3::new(0., f.extent + 10., -0.001),
                target: Vec3::ZERO,
                vertical_fov: 2.
                    * (f.extent / (4. * std::f32::consts::SQRT_2 * (f.extent + 10.))).atan(),
            }
        } else {
            CameraSpec::LookAt {
                position: Vec3::new(-4., 3., -4.),
                target: Vec3::ZERO,
                vertical_fov: 55_f32.to_radians(),
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Suite;
    #[test]
    fn every_authored_recipe_is_a_valid_native_scene_through_motion() {
        let suite: Suite = serde_json::from_str(include_str!(
            "../../../ara-dev/src/lab/stress/cases.v1.json"
        ))
        .unwrap();
        for case in suite.cases {
            // External licensed content is checked by the optional Polar run.
            if case.recipe == "polar" {
                continue;
            }
            let mut fixture = create(&case).unwrap();
            assert_eq!(
                fixture.scene.lights.len(),
                case.lights as usize,
                "{} authored light count",
                case.id
            );
            if case.recipe == "combined" {
                assert!(
                    fixture.scene.sun_lux.min_element() > 0.,
                    "combined stress must retain its low sun"
                );
            }
            fixture
                .scene
                .validate()
                .unwrap_or_else(|e| panic!("{} initial: {e}", case.id));
            let script = case.script(60);
            for frame in script.iter().filter(|s| {
                [
                    0,
                    case.event_frame,
                    (case.event_frame + 1).min(case.frames - 1),
                    case.frames - 1,
                ]
                .contains(&s.frame)
            }) {
                apply(&mut fixture, &case, frame);
                fixture
                    .scene
                    .validate()
                    .unwrap_or_else(|e| panic!("{} frame {}: {e}", case.id, frame.frame));
                fixture.camera.resolve([1920, 1080]).unwrap();
            }
        }
    }
}
