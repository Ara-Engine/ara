#![allow(clippy::unwrap_used)]
use ara_asset::{
    Cancellation, CookPreset, DEFAULT_THREAD_BUDGET, ImportContext, ImportInput, JobEvent,
    ProcessWorker, TargetPlatform,
};
use std::collections::BTreeMap;
fn input() -> ImportInput {
    let image = image::RgbaImage::from_fn(9, 7, |x, y| {
        image::Rgba([x as u8 * 20, y as u8 * 25, 100, 255])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    ImportInput {
        preset: CookPreset::default(),
        thread_budget: DEFAULT_THREAD_BUDGET,
        source_path: "sprite.png".into(),
        source: png.into_inner(),
        importer: "ara.atlas2d".into(),
        version: "1".into(),
        settings: serde_json::json!({"maxSize":32,"padding":2,"regions":[{"name":"right","region":{"x":5,"y":0,"width":4,"height":7}},{"name":"left","region":{"width":5,"height":7}}]}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    }
}
#[test]
fn pure_pack_extrudes_edges_rejects_bad_crops_and_honors_cancellation() {
    let registry = ara_import2d::registry().unwrap();
    let input = input();
    let token = Cancellation::default();
    let mut progress = |_| {};
    let mut ctx = ImportContext {
        metrics: None,
        cancellation: &token,
        progress: &mut progress,
    };
    let out = registry.import(&input, &mut ctx).unwrap();
    assert_eq!(out, registry.import(&input, &mut ctx).unwrap());
    let metadata = &out
        .artifacts
        .iter()
        .find(|a| a.name == "atlas.json")
        .unwrap()
        .bytes;
    let atlas: ara_2d::Atlas = serde_json::from_slice(metadata).unwrap();
    let pixels = &out
        .artifacts
        .iter()
        .find(|a| a.name == "atlas.rgba")
        .unwrap()
        .bytes;
    let region = &atlas.regions["left"];
    let pixel = |x, y| {
        let index = 16 + ((y * atlas.width + x) * 4) as usize;
        &pixels[index..index + 4]
    };
    assert_eq!(pixel(region.x - 1, region.y), pixel(region.x, region.y));
    assert_eq!(pixel(region.x, region.y - 1), pixel(region.x, region.y));
    let mut bad = input.clone();
    bad.settings["regions"][0]["region"]["x"] = serde_json::json!(99);
    assert!(registry.import(&bad, &mut ctx).is_err());
    token.cancel();
    assert!(registry.import(&input, &mut ctx).is_err());
}
#[test]
fn real_worker_matches_pure_output_and_cache_action_identity() {
    let input = input();
    let token = Cancellation::default();
    let mut progress = |_| {};
    let expected = ara_import2d::registry()
        .unwrap()
        .import(
            &input,
            &mut ImportContext {
                metrics: None,
                cancellation: &token,
                progress: &mut progress,
            },
        )
        .unwrap();
    let mut job = ProcessWorker::new(env!("CARGO_BIN_EXE_ara-import2d"))
        .with_timeout(std::time::Duration::from_secs(20))
        .start(input.clone())
        .unwrap();
    loop {
        match job.poll().unwrap() {
            Some(JobEvent::Complete(output)) => {
                assert_eq!(output, expected);
                break;
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(2)),
        }
    }
    let mut changed = input.clone();
    changed.settings["padding"] = serde_json::json!(3);
    assert_ne!(input.key().unwrap(), changed.key().unwrap());
    let mut db = ara_schema::SchemaDb::new();
    ara_import2d::registry().unwrap().register_schemas(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
}
#[test]
fn tilemap_cook_emits_canonical_manifest_and_content_addressed_chunks() {
    let map = serde_json::json!({
        "definitions": {"1": {"region": "grass", "solid": true}},
        "layers": [{"id": "l_ground", "cells": {"-1,0": 1, "40,3": 1}}]
    });
    let mut tilemap = input();
    tilemap.importer = "ara.tilemap2d".into();
    tilemap.version = "2".into();
    tilemap.source = serde_json::to_vec(&map).unwrap();
    tilemap.source_path = "level.tilemap".into();
    let token = Cancellation::default();
    let mut progress = |_| {};
    let out = ara_import2d::registry()
        .unwrap()
        .import(
            &tilemap,
            &mut ImportContext {
                metrics: None,
                cancellation: &token,
                progress: &mut progress,
            },
        )
        .unwrap();
    let names: Vec<_> = out.artifacts.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"tilemap.json"), "{names:?}");
    // One chunk per occupied 32x32 block: (-1,0) lands in chunk -1,0 and (40,3) in 1,0.
    let chunks: Vec<_> = out
        .artifacts
        .iter()
        .filter(|a| a.name.starts_with("chunks/l_ground/"))
        .collect();
    assert_eq!(chunks.len(), 2, "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("chunks/l_ground/-1,0.")));
    assert!(names.iter().any(|n| n.starts_with("chunks/l_ground/1,0.")));
    assert!(
        chunks
            .iter()
            .all(|a| a.bytes.starts_with(b"AR2C\x02") && a.bytes.len() == 16 + 4096)
    );
}
