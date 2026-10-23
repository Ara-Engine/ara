#[path = "../examples/common/mod.rs"]
mod common;
use anyhow::{Result, ensure};
use ara_render::capture::{ViewMode, compare};
use ara_render2d::Renderer2d;
use std::path::Path;

#[test]
fn sample_scene_is_exactly_its_command_script() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let authored = common::fixtures::author(&root.join("sample/platformer.commands.json"))?;
    let committed = std::fs::read_to_string(root.join("sample/platformer.scene"))?;
    ensure!(
        authored == committed,
        "platformer.scene drifted from platformer.commands.json; run `platformer prepare`"
    );
    Ok(())
}

#[test]
fn platformer_capture_goldens_picking_parallax_and_lighting() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (scene, camera) = common::scene(
        root,
        Some(Path::new(env!("CARGO_BIN_EXE_ara-2d-import-worker"))),
    )?;
    let mut renderer = Renderer2d::new(&scene, [640, 360], root.join("../../.sn/test2d"))?;
    let update = std::env::var("ARA_UPDATE_2D_GOLDENS").as_deref() == Ok("1");
    let dir = root.join("tests/goldens");
    let mut lit = None;
    for mode in [
        ViewMode::Lit,
        ViewMode::Albedo,
        ViewMode::Normals,
        ViewMode::EntityIds,
        ViewMode::ShadowCascades,
    ] {
        let mut request = common::request([640, 360]);
        request.mode = mode;
        let capture = renderer.capture(&camera, request)?;
        let path = dir.join(format!("{}.png", mode.name()));
        if update {
            std::fs::create_dir_all(&dir)?;
            capture.image.save(&path)?;
            if matches!(mode, ViewMode::Lit) {
                capture.write(&dir.join("platformer.png"))?;
            }
        }
        let reference = image::open(&path)?.to_rgba8();
        let comparison = compare(&capture.image, &reference, 0.005)?;
        if !comparison.passed {
            std::fs::create_dir_all(root.join("captures"))?;
            comparison.diff.save(root.join("captures/diff.png"))?;
        }
        ensure!(
            comparison.passed,
            "{} SSIM error {}",
            mode.name(),
            comparison.score
        );
        ensure!(capture.legend.len() >= 12, "missing visible scene entities");
        for item in &capture.legend {
            let [x, y, w, h] = item.bbox;
            ensure!(
                (y..y + h)
                    .any(|py| (x..x + w).any(|px| capture.entity_at(px, py) == Some(item.entity))),
                "pick mismatch"
            );
        }
        if matches!(mode, ViewMode::Lit) {
            lit = Some(capture.image);
        }
    }
    ensure!(
        Some(
            renderer
                .capture(&camera, common::request([640, 360]))?
                .image
        ) == lit,
        "capture depends on history"
    );
    let mut moved = camera.clone();
    moved.centre[0] += 1.;
    ensure!(
        Some(renderer.capture(&moved, common::request([640, 360]))?.image) != lit,
        "camera/parallax did not move"
    );
    renderer.gpu().assert_clean()?;
    let mut unlit = scene.clone();
    let mut no_lights = scene.clone();
    no_lights.lights.clear();
    let mut dark = Renderer2d::new(&no_lights, [640, 360], root.join("../../.sn/test2d-dark"))?;
    ensure!(
        Some(dark.capture(&camera, common::request([640, 360]))?.image) != lit,
        "local lights made no visible contribution"
    );
    drop(dark);
    for sprite in &mut unlit.sprites {
        sprite.sprite.lit = false;
    }
    for map in &mut unlit.tilemaps {
        for definition in map.map.definitions.values_mut() {
            definition.lit = false;
        }
    }
    let mut a = Renderer2d::new(&unlit, [640, 360], root.join("../../.sn/test2d-unlit"))?;
    let independent = a.capture(&camera, common::request([640, 360]))?.image;
    drop(a);
    unlit.lights.clear();
    let mut b = Renderer2d::new(&unlit, [640, 360], root.join("../../.sn/test2d-unlit"))?;
    ensure!(
        independent == b.capture(&camera, common::request([640, 360]))?.image,
        "unlit sprites paid for/depended on opt-in lights"
    );
    Ok(())
}
