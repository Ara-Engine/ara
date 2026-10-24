//! Native generations share the renderer's authored environment and IBL pipeline.
use super::*;

pub(super) fn snapshot(scene: &NativeScene) -> Scene {
    Scene {
        meshes: Vec::new(),
        materials: vec![crate::scene::Material {
            textures: [0; 4],
            ..Default::default()
        }],
        textures: vec![crate::scene::Texture {
            size: [1, 1],
            rgba: vec![255; 4],
            srgb: false,
        }],
        lights: scene.lights.to_vec(),
        sun_direction: scene.sun_direction,
        sun_lux: scene.sun_lux,
        environment: scene.environment.to_vec(),
        environment_size: scene.environment_size,
    }
}
