//! Glint's numbers for the three-engine comparison: `compare/glint.spec.json` (exposure, sky,
//! sun, lights, emissive levels, the palette) and `compare/camera_path.json` (the review stills
//! and one fixed fly-over). Both come from the tables the scene is built from.
use super::{EV100, SUN, View, kits};
use crate::build::place::round3;
use anyhow::Result;
use glam::{Mat3, Quat, Vec3};
use serde_json::{Value, json};
use std::path::Path;

/// A camera looks along -Z: its rotation as a glTF quaternion `[x, y, z, w]`.
fn look_rotation(position: Vec3, target: Vec3) -> [f32; 4] {
    let forward = (target - position).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    Quat::from_mat3(&Mat3::from_cols(right, up, -forward))
        .normalize()
        .to_array()
}

fn key(t: f32, view: &View) -> Value {
    json!({"t": t, "position": round3(view.at), "rotation": look_rotation(view.at, view.look),
        "lookAt": round3(view.look), "fovYDeg": view.fov, "cut": false})
}

/// The stills (one clip each, duration 0) and the fly-over through them, 6 s a leg.
fn path(views: &[View]) -> Value {
    let mut clips: Vec<Value> = views
        .iter()
        .map(|v| json!({"name": v.id.trim_start_matches("e_cam"), "note": v.name, "duration": 0, "keys": [key(0.0, v)]}))
        .collect();
    let keys: Vec<Value> = views
        .iter()
        .enumerate()
        .map(|(i, v)| key(i as f32 * 6.0, v))
        .collect();
    clips.push(json!({
        "name": "flyover",
        "note": "One fixed pass over the level: from behind the start, by the turn, the sliders, the cannon, to the goal.",
        "duration": (views.len() - 1) as f32 * 6.0,
        "keys": keys,
    }));
    json!({
        "handedness": "right", "up": "+Y", "units": "metres", "cameraForward": "-Z (glTF)",
        "fps": 30, "resolution": [1920, 1080],
        "interpolation": "position: linear; rotation: slerp",
        "clips": clips,
    })
}

/// The scene's light and material numbers. `environment` and `fog` are the scene's own blocks.
fn spec(environment: &Value, fog: &Value, lights: &[Value], emissive: &Value) -> Value {
    let palette: Vec<Value> = kits::palette()
        .iter()
        .map(|m| json!({"name": m.name, "baseColor": m.color, "roughness": m.roughness, "metallic": m.metallic}))
        .collect();
    json!({
        "exposure": {"ev100": EV100, "auto": false, "toneMapper": "Khronos PBR Neutral", "bloom": 0.16},
        "sun": {"illuminanceLux": 95_000, "color": [1.0, 0.88, 0.72], "travels": round3(SUN), "angularDiameterDeg": 0.5357,
            "note": "20 degrees above the horizon; shadows on"},
        "environment": environment,
        "fog": fog,
        "lights": lights,
        "emissiveNits": emissive,
        "palette": palette,
        "notes": [
            "Stone and paint are matte; the polish is the navy inlay, the pad discs and the goal's floor.",
            "The guard glass is transmission glass (IOR 1.5, thin wall).",
            "Linear colours; lengths in metres; lights are point lights in lumens with a source radius.",
        ],
    })
}

/// Writes both files into `dir`.
pub fn write(
    dir: &Path,
    views: &[View],
    environment: &Value,
    fog: &Value,
    lights: &[Value],
    emissive: &Value,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join("camera_path.json"),
        ara_json::to_canonical_string(&path(views)),
    )?;
    std::fs::write(
        dir.join("glint.spec.json"),
        ara_json::to_canonical_string(&spec(environment, fog, lights, emissive)),
    )?;
    Ok(())
}
