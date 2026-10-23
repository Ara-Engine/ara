#![allow(clippy::unwrap_used)]
#[path = "../examples/common/mod.rs"]
mod common;
use anyhow::{Result, ensure};
use ara_render::{Renderer, capture::compare};
use glam::Vec3;
use std::path::Path;

/// Moving geometry, live light edits and scene replacement between frames, with
/// validation (including synchronization validation) clean throughout.
#[test]
fn dynamic_updates_and_scene_replacement_are_visible_and_validation_clean() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let size = [320, 180];
    let mut renderer = Renderer::new(
        common::scene()?,
        size,
        &[],
        root.join("../../.sn/test-render"),
    )?;
    let request = common::request(size);
    let original = renderer.capture(&request)?;
    let sphere = 200;
    let before = original.legend.iter().find(|m| m.entity == sphere).unwrap();
    let mut moved = common::scene()?
        .meshes
        .into_iter()
        .find(|m| m.entity == sphere)
        .unwrap()
        .vertices;
    for vertex in &mut moved {
        vertex.position[1] += 1.5;
    }
    renderer.update_mesh(sphere, &moved)?;
    renderer.update_mesh(sphere, &moved)?;
    let lifted = renderer.capture(&request)?;
    let after = lifted.legend.iter().find(|m| m.entity == sphere).unwrap();
    ensure!(
        after.bbox[1] < before.bbox[1],
        "Updated vertices were not drawn"
    );
    ensure!(
        lifted.metadata.scene_hash != original.metadata.scene_hash,
        "Provenance must cover dynamic updates"
    );
    ensure!(renderer.update_mesh(sphere, &moved[1..]).is_err());
    ensure!(renderer.update_mesh(9999, &moved).is_err());

    let mut lights = common::scene()?.lights;
    for light in &mut lights {
        light.color_lumens = [0.1, 1.0, 0.1, 40_000.];
    }
    renderer.set_lights(&lights)?;
    renderer.set_sun(Vec3::new(0.3, 1., -0.2), Vec3::splat(900.))?;
    let relit = renderer.capture(&request)?;
    ensure!(
        !compare(&relit.image, &lifted.image, 0.005)?.passed,
        "Light edits were not visible"
    );
    ensure!(renderer.set_lights(&vec![lights[0]; 65]).is_err());

    renderer.replace_scene(common::scene()?)?;
    let replaced = renderer.capture(&request)?;
    ensure!(
        replaced.image == original.image,
        "Replacement must equal a fresh upload"
    );
    ensure!(replaced.metadata.scene_hash == original.metadata.scene_hash);
    renderer.gpu().assert_clean()?;
    Ok(())
}
