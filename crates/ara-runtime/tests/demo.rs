#![allow(clippy::unwrap_used)]
//! The demo scene end to end in-process: player movement, AI crowds, animation,
//! determinism, exact checkpoints, portable snapshots, live patches and keep-changes.
use ara_asset::{CachedImport, Cancellation, ImportContext, ImportInput, TargetPlatform};
use ara_doc::Workspace;
use ara_input::{ActionValue, Injection, PlayerId};
use ara_runtime::{
    Runtime, RuntimeConfig,
    assets::MemoryAssets,
    extract::{Geometry, RenderFrame},
    live::set_edit,
};
use ara_sim::{Snapshot, StepInput};
use serde_json::{Value, json};
use std::{path::Path, sync::OnceLock};

fn demo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../ara-player/demo"))
}

/// The Fox, cooked once in-process (the player loads the same artifacts from cache).
fn fox() -> &'static CachedImport {
    static FOX: OnceLock<CachedImport> = OnceLock::new();
    FOX.get_or_init(|| {
        let registry = ara_importers::registry().unwrap();
        let input = ImportInput {
            preset: ara_asset::CookPreset::Fast,
            thread_budget: 4,
            source_path: "assets/fox/Fox.glb".into(),
            source: std::fs::read(demo().join("assets/fox/Fox.glb")).unwrap(),
            importer: "ara.gltf".into(),
            version: "2-gltf14-meshopt070-ispc050-tiled".into(),
            settings: Value::Null,
            target: TargetPlatform::Windows,
            dependencies: Default::default(),
            asset_dependencies: Default::default(),
        };
        let cancellation = Cancellation::default();
        let mut progress = |_| {};
        let mut context = ImportContext {
            metrics: None,
            cancellation: &cancellation,
            progress: &mut progress,
        };
        let output = registry.import(&input, &mut context).unwrap();
        CachedImport {
            key: input.key().unwrap(),
            output,
        }
    })
}

fn runtime(seed: u64) -> Runtime {
    let schema = ara_runtime::schema(&[]).unwrap();
    let text = std::fs::read_to_string(demo().join("levels/demo.scene")).unwrap();
    let read = ara_doc::read(&text, "levels/demo.scene", &schema, None);
    assert!(read.diags.iter().all(|d| !d.is_error()), "{:?}", read.diags);
    let doc = read.doc.unwrap();
    let scene = doc.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("levels/demo.scene", doc);
    let assets = MemoryAssets([("as_fox".to_owned(), fox().clone())].into());
    Runtime::new(
        schema,
        workspace,
        scene,
        Box::new(assets),
        RuntimeConfig {
            seed,
            ..Default::default()
        },
        Vec::new(),
    )
    .unwrap()
}

fn walk(ticks: u32) -> Vec<StepInput> {
    vec![StepInput {
        after_ticks: 0,
        input: Injection {
            player: PlayerId(1),
            action: "Move".into(),
            value: ActionValue::Axis2([0.0, 1.0]),
            hold_ticks: ticks,
        },
    }]
}

fn component(rt: &Runtime, entity: &str, name: &str) -> Value {
    rt.play()
        .session()
        .component(entity, name)
        .unwrap()
        .unwrap()
}

#[test]
fn player_moves_grounded_and_agents_roam() {
    let mut rt = runtime(7);
    let start = component(&rt, "e_fox", "Transform")["position"].clone();
    let bot = component(&rt, "e_bot1", "Transform")["position"].clone();
    rt.step(120, &walk(120)).unwrap();
    let end = component(&rt, "e_fox", "Transform")["position"].clone();
    assert!(
        end[2].as_f64().unwrap() - start[2].as_f64().unwrap() > 1.0,
        "fox walked forward: {start} -> {end}"
    );
    assert_eq!(
        component(&rt, "e_fox", "CharacterMotion")["grounded"],
        json!(true)
    );
    assert_ne!(
        component(&rt, "e_bot1", "Transform")["position"],
        bot,
        "wanderer moved"
    );
    assert!(
        rt.navigation_errors().is_empty(),
        "{:?}",
        rt.navigation_errors()
    );
    assert!(rt.fault().is_none());

    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    assert!(frame.camera.is_some() && frame.sun.is_some() && frame.lights.len() == 4);
    let skinned = frame
        .draws
        .iter()
        .find(|d| matches!(d.geometry, Geometry::Model { .. }))
        .unwrap();
    let range = skinned.skin.clone().unwrap();
    assert_eq!(range.len(), 24, "fox palette in skin order");
    assert!(frame.palettes[range].iter().all(|m| m.is_finite()));
}

#[test]
fn equal_seed_and_inputs_give_equal_hashes_and_checkpoints_are_exact() {
    let mut a = runtime(3);
    let mut b = runtime(3);
    a.step(90, &walk(90)).unwrap();
    b.step(90, &walk(90)).unwrap();
    let hash = a.play().session().state_hash().unwrap();
    assert_eq!(hash, b.play().session().state_hash().unwrap());

    let exact = a.play_mut().checkpoint().unwrap();
    a.step(60, &walk(60)).unwrap();
    let ahead = a.play().session().state_hash().unwrap();
    a.play_mut().restore(&exact).unwrap();
    assert_eq!(a.play().session().state_hash().unwrap(), hash);
    a.step(60, &walk(60)).unwrap();
    assert_eq!(
        a.play().session().state_hash().unwrap(),
        ahead,
        "exact native rollback"
    );

    // A portable tick-0 snapshot rebuilds native state and replays identically.
    let mut fresh = runtime(3);
    let bytes = fresh.play().snapshot().unwrap().to_bytes().unwrap();
    fresh.step(30, &walk(30)).unwrap();
    let thirty = fresh.play().session().state_hash().unwrap();
    fresh
        .play_mut()
        .restore(&Snapshot::from_bytes(&bytes).unwrap())
        .unwrap();
    fresh.step(30, &walk(30)).unwrap();
    assert_eq!(fresh.play().session().state_hash().unwrap(), thirty);
}

#[test]
fn live_patches_apply_at_the_next_tick_and_keep_changes_reports_them() {
    let mut rt = runtime(1);
    rt.step(5, &[]).unwrap();
    let scene = rt.play().session().projection().scene().clone();
    let edit = set_edit(
        &scene,
        "e_lamp1",
        "Light.color",
        Some(json!([0.25, 1.0, 0.5, 1])),
    )
    .unwrap();
    let ops = rt.patch_edits(vec![edit]).unwrap();
    assert_eq!(ops.len(), 1);
    assert_ne!(
        component(&rt, "e_lamp1", "Light")["color"],
        json!([0.25, 1.0, 0.5, 1.0])
    );
    rt.step(1, &[]).unwrap();
    assert_eq!(
        component(&rt, "e_lamp1", "Light")["color"],
        json!([0.25, 1.0, 0.5, 1.0])
    );
    let changes = rt.changes().unwrap();
    let kept: Vec<_> = changes.iter().filter(|c| !c.driven).collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(
        (kept[0].entity.as_str(), kept[0].path.as_str()),
        ("e_lamp1", "Light.color")
    );
    assert!(
        changes
            .iter()
            .any(|c| c.driven && c.entity.starts_with("e_bot"))
    );
}
