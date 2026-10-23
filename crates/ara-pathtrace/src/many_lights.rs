//! A deterministic 1,000-light corridor, including real emissive geometry.
use crate::scenes::{mat, quad};
use ara_render::scene::{CameraSpec, Light, Scene};
use glam::Vec3;

/// Five hundred point lights plus five hundred emissive panels; no sun or sky.
pub fn scene() -> Scene {
    let mut scene = crate::scenes::scene("glossy_interior", 0);
    scene.meshes.clear();
    scene.lights.clear();
    scene.sun_lux = Vec3::ZERO;
    scene.environment.fill([0., 0., 0., 1.]);
    scene.materials = vec![mat([0.6, 0.58, 0.53], 0.7, 0.)];
    let mut emitter = mat([1.; 3], 0.7, 0.);
    emitter.emissive_roughness = [8000., 6000., 4000., 0.7];
    scene.materials.push(emitter);
    for (p, u, v) in [
        (
            Vec3::new(-2., 0., 5.),
            Vec3::new(4., 0., 0.),
            Vec3::new(0., 0., -30.),
        ),
        (
            Vec3::new(-2., 3., -25.),
            Vec3::new(4., 0., 0.),
            Vec3::new(0., 0., 30.),
        ),
        (
            Vec3::new(-2., 0., -25.),
            Vec3::new(0., 0., 30.),
            Vec3::new(0., 3., 0.),
        ),
        (
            Vec3::new(2., 0., 5.),
            Vec3::new(0., 0., -30.),
            Vec3::new(0., 3., 0.),
        ),
        (
            Vec3::new(-2., 0., -25.),
            Vec3::new(4., 0., 0.),
            Vec3::new(0., 3., 0.),
        ),
    ] {
        scene
            .meshes
            .push(quad(scene.meshes.len() as u64 + 1, "corridor", p, u, v, 0));
    }
    for i in 0..500 {
        let x = -1.8 + (i % 10) as f32 * 0.4;
        let z = 4.5 - (i / 10) as f32 * 0.58;
        scene.lights.push(Light {
            position_range: [x, 2.65, z, 3.5],
            color_lumens: [0.55, 0.75, 1., 20.],
            direction_outer: [0., -1., 0., -1.],
            parameters: [0.; 4],
        });
        scene.meshes.push(quad(
            scene.meshes.len() as u64 + 1,
            "emissive panel",
            Vec3::new(x - 0.02, 2.95, z - 0.02),
            Vec3::new(0.04, 0., 0.),
            Vec3::new(0., 0., 0.04),
            1,
        ));
    }
    scene
}
/// Same corridor view for hybrid and independently sampled reference.
pub fn camera() -> CameraSpec {
    CameraSpec::LookAt {
        position: Vec3::new(0., 1.55, 4.8),
        target: Vec3::new(0., 1.5, -16.),
        vertical_fov: 0.95,
    }
}
