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

#[test]
fn update_matches_a_fresh_upload_and_rejects_atlas_changes() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (scene, camera) = common::scene(
        root,
        Some(Path::new(env!("CARGO_BIN_EXE_ara-2d-import-worker"))),
    )?;
    let mut renderer = Renderer2d::new(&scene, [640, 360], root.join("../../.sn/test2d-update"))?;
    let before = renderer
        .capture(&camera, common::request([640, 360]))?
        .image;
    // A gameplay frame: the scout walks, and a stroke adds many tiles (buffer growth).
    let mut next = scene.clone();
    let hero = next
        .sprites
        .iter_mut()
        .find(|s| s.name == "Scout")
        .expect("sample scout");
    hero.position[0] += 2.;
    let ground = next.tilemaps[0]
        .map
        .layers
        .iter_mut()
        .find(|l| l.id == "l_ruins")
        .expect("ruins layer");
    for x in -40..40 {
        for y in 8..16 {
            ground.cells.insert(ara_2d::tilemap::key(x, y), 3);
        }
    }
    renderer.update(&next)?;
    ensure!(renderer.instance_count() > 900, "stroke instances resident");
    let updated = renderer.capture(&camera, common::request([640, 360]))?;
    ensure!(updated.image != before, "update changed nothing");
    let mut fresh = Renderer2d::new(&next, [640, 360], root.join("../../.sn/test2d-update"))?;
    let reference = fresh.capture(&camera, common::request([640, 360]))?;
    ensure!(
        updated.image == reference.image,
        "update differs from a fresh upload"
    );
    ensure!(
        updated.metadata.scene_hash == reference.metadata.scene_hash,
        "provenance must describe resident content"
    );
    // A rejected update (gameplay wrote an invalid light) changes nothing resident.
    let mut broken = scene.clone();
    broken.lights[0].radius = 0.;
    ensure!(renderer.update(&broken).is_err(), "invalid light accepted");
    let after = renderer.capture(&camera, common::request([640, 360]))?;
    ensure!(
        after.image == updated.image && after.metadata.scene_hash == updated.metadata.scene_hash,
        "a rejected update left partial state resident"
    );
    let mut other = next.clone();
    if let Some(atlas) = other.atlases.values_mut().next() {
        atlas.atlas.regions.clear();
    }
    ensure!(
        renderer.update(&other).is_err(),
        "atlas metadata change must restage"
    );
    other.atlases.clear();
    ensure!(
        renderer.update(&other).is_err(),
        "atlas set change must restage"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn tall_isometric_tiles_draw_back_to_front() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (sample, _) = common::scene(
        root,
        Some(Path::new(env!("CARGO_BIN_EXE_ara-2d-import-worker"))),
    )?;
    let tall = |region: &str| ara_2d::TileDefinition {
        region: region.into(),
        size: [1., 1.5],
        ..Default::default()
    };
    let map = ara_2d::Tilemap {
        atlas: Some(ara_schema::AssetRef(common::ATLAS.into())),
        layout: ara_2d::GridLayout::Isometric,
        cell_size: [1., 0.5],
        definitions: [("1".into(), tall("stone")), ("2".into(), tall("crate"))].into(),
        layers: vec![ara_2d::TileLayer {
            id: "l_iso".into(),
            // "0,0" sorts before "1,1" as text, yet (0,0) is nearer and must win.
            // (30,-1) and (31,-1) straddle a 32-cell column band; (30,-1) is nearer.
            cells: [
                ("0,0".into(), 1),
                ("1,1".into(), 2),
                ("30,-1".into(), 1),
                ("31,-1".into(), 2),
            ]
            .into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let scene = ara_render2d::Scene2d {
        atlases: sample.atlases,
        tilemaps: vec![ara_render2d::TilemapDraw {
            entity: 1,
            name: "Iso".into(),
            origin: [0.; 2],
            basis: [1., 0., 0., 1.],
            map,
        }],
        ..Default::default()
    };
    let mut renderer = Renderer2d::new(&scene, [128, 128], root.join("../../.sn/test2d-iso"))?;
    let camera = ara_2d::Camera2d {
        centre: [0., 0.25],
        zoom: 4.,
        ..Default::default()
    };
    let mut request = common::request([128, 128]);
    request.mode = ViewMode::Albedo;
    request.marks = None;
    // Each camera centre lies in both quads; masonry (blue) must cover the crate (brown).
    for centre in [[0., 0.25], [15.75, 7.]] {
        let camera = ara_2d::Camera2d {
            centre,
            ..camera.clone()
        };
        let capture = renderer.capture(&camera, request.clone())?;
        let [r, _, b, _] = capture.image.get_pixel(64, 64).0;
        ensure!(
            b > r,
            "back tile drawn over the front tile at {centre:?}: rgb {r},{b}"
        );
    }
    Ok(())
}
