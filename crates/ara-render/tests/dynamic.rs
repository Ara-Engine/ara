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
    ensure!(renderer.entity_at([0, 0])?.is_none());
    let original = renderer.capture(&request)?;
    ensure!(
        renderer
            .timings()
            .iter()
            .filter(|p| p.pass == "CaptureReadback")
            .count()
            == 1,
        "Capture copies its final frame once, after settling"
    );
    let pick = (0..size[1])
        .find_map(|y| {
            (0..size[0]).find_map(|x| original.entity_at(x, y).map(|entity| ([x, y], entity)))
        })
        .expect("visible test geometry");
    ensure!(renderer.entity_at(pick.0)? == Some(pick.1));
    renderer.render(&request.camera, request.exposure)?;
    ensure!(
        !renderer
            .timings()
            .iter()
            .any(|p| p.pass == "CaptureReadback"),
        "Live frames must not copy output or IDs to host memory"
    );
    let live_timings = serde_json::to_vec_pretty(renderer.timings())?;
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

    let mut edited = common::scene()?;
    edited.materials[0].base_color = [0.02, 0.8, 0.05, 1.];
    edited.materials[0].textures[0] = 0;
    let sphere_material = edited
        .meshes
        .iter()
        .find(|m| m.entity == sphere)
        .unwrap()
        .material;
    edited.materials[sphere_material].flags[0] = 2;
    edited.materials[sphere_material].base_color[3] = 0.35;
    let output = renderer.output();
    let ids = renderer.entity_channel();
    renderer.update_materials(edited.materials.clone())?;
    renderer.rename(sphere, "Edited sphere".into())?;
    let parameters = renderer.capture(&request)?;
    ensure!(renderer.output() == output && renderer.entity_channel() == ids);
    ensure!(
        parameters.image != original.image,
        "Material update was not visible"
    );
    ensure!(parameters.metadata.scene_hash != original.metadata.scene_hash);
    ensure!(
        parameters
            .legend
            .iter()
            .any(|m| m.entity == sphere && m.name == "Edited sphere")
    );
    ensure!(renderer.rename(9999, "Unknown".into()).is_err());

    let mut invalid = edited.materials.clone();
    invalid[0].surface[0] = f32::NAN;
    ensure!(renderer.update_materials(invalid).is_err());
    let mut invalid = edited.materials.clone();
    invalid[0].textures[0] = u32::MAX;
    ensure!(renderer.update_materials(invalid).is_err());
    ensure!(
        renderer
            .update_materials(edited.materials[1..].to_vec())
            .is_err()
    );
    let retained = renderer.capture(&request)?;
    ensure!(retained.image == parameters.image);
    ensure!(retained.metadata.scene_hash == parameters.metadata.scene_hash);

    edited
        .meshes
        .iter_mut()
        .find(|m| m.entity == sphere)
        .unwrap()
        .name = "Edited sphere".into();
    renderer.replace_scene(edited)?;
    let fresh = renderer.capture(&request)?;
    ensure!(
        fresh.image == parameters.image,
        "Material slots and opaque-to-transparent edits must match a fresh scene upload"
    );
    let evidence = root.join("../../target/ara-render/dynamic");
    std::fs::create_dir_all(&evidence)?;
    std::fs::write(evidence.join("live-timings.json"), live_timings)?;
    original.write(&evidence.join("original.png"))?;
    parameters.write(&evidence.join("materials-edited.png"))?;
    renderer.gpu().assert_clean()?;
    Ok(())
}
