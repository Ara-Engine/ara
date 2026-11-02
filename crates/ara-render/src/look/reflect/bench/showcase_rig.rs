//! Authored Showcase camera, lights and environment, with the lab's fixed post.
use super::fixture::Fixture;
use anyhow::{Context, Result, ensure};
use ara_render::scene::{CameraSpec, Light};
use glam::{EulerRot, Quat, Vec3};
use serde_json::Value;
use std::{fs, path::Path};

fn vec3(value: &Value, fallback: [f32; 3]) -> Vec3 {
    Vec3::from_array(std::array::from_fn(|i| {
        value[i].as_f64().map_or(fallback[i], |v| v as f32)
    }))
}

pub fn apply(project: &Path, f: &mut Fixture) -> Result<()> {
    let doc: Value = serde_json::from_slice(&fs::read(project.join("content/levels/main.scene"))?)?;
    let records = doc["records"].as_object().context("Showcase records")?;
    let shot = std::env::var("ARA_B4_SHOWCASE_SHOT").unwrap_or_else(|_| "Shot_stage".into());
    let mut camera_found = false;
    for r in records.values() {
        let p = vec3(&r["Transform"]["position"], [0.; 3]);
        let angles = vec3(&r["Transform"]["rotation"], [0.; 3]) * std::f32::consts::PI / 180.;
        let rotation = Quat::from_euler(EulerRot::XYZ, angles.x, angles.y, angles.z);
        let forward = rotation * -Vec3::Z;
        if r["name"] == shot && r.get("Camera").is_some() {
            f.camera = CameraSpec::LookAt {
                position: p,
                target: p + forward,
                vertical_fov: (r["Camera"]["fov"].as_f64().context("Showcase FOV")? as f32)
                    .to_radians(),
            };
            camera_found = true;
        }
        if let Some(light) = r.get("Light") {
            let color = vec3(&light["color"], [1.; 3]);
            let intensity = light["intensity"]
                .as_f64()
                .context("Showcase light intensity")? as f32;
            if light["kind"] == "directional" {
                f.scene.sun_direction = -forward;
                f.scene.sun_lux = color * intensity;
            } else {
                ensure!(light["kind"] == "spot", "unexpected Showcase light kind");
                f.scene.lights.push(Light {
                    position_range: [
                        p.x,
                        p.y,
                        p.z,
                        light["range"].as_f64().context("Showcase light range")? as f32,
                    ],
                    color_lumens: [color.x, color.y, color.z, intensity],
                    direction_outer: [
                        forward.x,
                        forward.y,
                        forward.z,
                        (light["outerAngle"]
                            .as_f64()
                            .context("Showcase outer angle")? as f32)
                            .to_radians()
                            .cos(),
                    ],
                    parameters: [
                        1.,
                        (light["innerAngle"]
                            .as_f64()
                            .context("Showcase inner angle")? as f32)
                            .to_radians()
                            .cos(),
                        0.,
                        1.,
                    ],
                    ..Light::ZERO
                });
            }
        }
        if let Some(env) = r.get("Environment") {
            let sky = vec3(&env["sky"], [0.5, 0.48, 0.45]);
            let horizon = vec3(&env["horizon"], [0.6, 0.55, 0.5]);
            let ground = vec3(&env["ground"], [0.25, 0.22, 0.2]);
            f.scene.environment_size = [64, 128];
            f.scene.environment.clear();
            for y in 0..128 {
                let s = (std::f32::consts::FRAC_PI_2
                    - (y as f32 + 0.5) / 128. * std::f32::consts::PI)
                    .sin();
                let c = if s >= 0. {
                    sky.lerp(horizon, (1. - s).powi(3))
                } else {
                    horizon.lerp(ground, (-s * 4.).min(1.))
                };
                let c = c * env["intensity"].as_f64().unwrap_or(900.) as f32;
                f.scene
                    .environment
                    .extend(std::iter::repeat_n([c.x, c.y, c.z, 1.], 64));
            }
        }
    }
    ensure!(camera_found, "Showcase camera {shot} not found");
    f.scene.validate()
}
