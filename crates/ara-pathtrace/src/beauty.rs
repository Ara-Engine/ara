//! Pinned CC0 furniture arranged as a photographic interior; fetched assets stay local.
use crate::scenes::{mat, quad};
use anyhow::Result;
use ara_render::scene::{CameraSpec, Scene};
use glam::{Mat4, Vec3};
use std::path::Path;

/// Import the authored glTF base-colour, normal and metallic/roughness channels.
pub fn scene(path: &Path) -> Result<Scene> {
    let mut scene = crate::scenes::scene("glossy_interior", 0);
    scene.meshes.clear();
    scene.lights.clear();
    scene.materials = vec![
        mat([0.48, 0.32, 0.18], 0.45, 0.),
        mat([0.65, 0.63, 0.58], 0.85, 0.),
    ];
    for (name, p, u, v, m) in [
        (
            "floor",
            Vec3::new(-4., 0., -3.),
            Vec3::new(0., 0., 8.),
            Vec3::new(8., 0., 0.),
            0,
        ),
        (
            "back plaster",
            Vec3::new(-4., 0., -3.),
            Vec3::new(8., 0., 0.),
            Vec3::new(0., 3.5, 0.),
            1,
        ),
        (
            "left plaster below window",
            Vec3::new(-4., 0., -3.),
            Vec3::new(0., 0., 8.),
            Vec3::new(0., 0.7, 0.),
            1,
        ),
        (
            "left plaster above window",
            Vec3::new(-4., 2.8, -3.),
            Vec3::new(0., 0., 8.),
            Vec3::new(0., 0.7, 0.),
            1,
        ),
        (
            "ceiling",
            Vec3::new(-4., 3.5, -3.),
            Vec3::new(8., 0., 0.),
            Vec3::new(0., 0., 8.),
            1,
        ),
    ] {
        scene
            .meshes
            .push(quad(scene.meshes.len() as u64 + 1, name, p, u, v, m));
    }
    for (id, position, angle) in [
        ("sofa_03", Vec3::new(0., 0., -1.9), 0.),
        ("mid_century_lounge_chair", Vec3::new(-2.3, 0., 0.25), 0.7),
        ("coffee_table_round_01", Vec3::new(0., 0., 0.1), 0.),
        ("potted_plant_04", Vec3::new(0.1, 0.49, 0.1), 0.),
    ] {
        crate::gltf_fixture::append(
            &mut scene,
            &path.join(id),
            Mat4::from_translation(position) * Mat4::from_rotation_y(angle),
        )?;
    }
    for (slot, name, srgb) in [
        (0, "diff.png", true),
        (1, "normal.png", false),
        (2, "orm.png", false),
    ] {
        let image = image::open(path.join("wood_floor").join(name))?.to_rgba8();
        let index = scene.textures.len() as u32;
        scene.textures.push(ara_render::scene::Texture {
            size: [image.width(), image.height()],
            rgba: image.into_raw(),
            srgb,
        });
        scene.materials[0].textures[slot] = index;
    }
    scene.materials[0].base_color = [1.; 4];
    scene.materials[0].emissive_roughness[3] = 1.;
    scene.materials[0].surface[0] = 1.;
    for vertex in &mut scene.meshes[0].vertices {
        vertex.uv[0] *= 4.;
        vertex.uv[1] *= 4.;
    }
    scene.sun_direction = Vec3::new(-0.7, 0.65, 0.3).normalize();
    scene.sun_lux = Vec3::new(18000., 16500., 14000.);
    scene.environment = vec![[60., 85., 130., 1.]; 8];
    Ok(scene)
}
/// Fixed view of the CC0 furniture and its contact/indirect lighting.
pub fn camera() -> CameraSpec {
    CameraSpec::LookAt {
        position: Vec3::new(2.2, 1.45, 3.),
        target: Vec3::new(-0.1, 0.7, -1.2),
        vertical_fov: 0.63,
    }
}
