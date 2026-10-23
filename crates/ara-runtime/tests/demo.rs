#![allow(clippy::unwrap_used)]
//! The demo scene end to end in-process: player movement, AI crowds, animation
//! (`.animation` controllers, motion matching), determinism, exact checkpoints,
//! portable snapshots, live patches and keep-changes.
use ara_asset::{CachedImport, Cancellation, ImportContext, ImportInput, TargetPlatform};
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_input::{ActionValue, Injection, PlayerId};
use ara_runtime::{
    Runtime, RuntimeConfig,
    animation::AnimatorSlot,
    assets::MemoryAssets,
    extract::{Geometry, RenderFrame},
    live::set_edit,
};
use ara_schema::{AssetRef, SchemaDb};
use ara_sim::{Identity, Snapshot, StepInput};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::OnceLock};

/// The demo's animation document (project path, document ID): one file holding the
/// Fox's clips and controllers, referenced per record (`@anim/fox.animation#an_walk`).
const ANIMATION: (&str, &str) = ("anim/fox.animation", "ac_foxanim1");

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
            version: registry.get("ara.gltf").unwrap().version().into(),
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

/// Reads a demo document strictly (canonical form, every engine type registered).
fn read(path: &str, text: &str, schema: &SchemaDb) -> Document {
    let read = ara_doc::read(text, path, schema, None);
    assert!(read.diags.iter().all(|d| !d.is_error()), "{:?}", read.diags);
    read.doc.unwrap()
}

fn runtime(seed: u64) -> Runtime {
    runtime_with(seed, |scene| scene)
}

/// The demo with its scene text rewritten by `edit` (variants of the same level).
fn runtime_with(seed: u64, edit: impl Fn(String) -> String) -> Runtime {
    let schema = ara_runtime::schema(&[]).unwrap();
    let text = edit(std::fs::read_to_string(demo().join("levels/demo.scene")).unwrap());
    let doc = read("levels/demo.scene", &text, &schema);
    let scene = doc.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("levels/demo.scene", doc);
    let (path, _) = ANIMATION;
    let text = std::fs::read_to_string(demo().join(path)).unwrap();
    workspace.insert(path, read(path, &text, &schema));
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

/// The demo's animation document, authored through typed `AnimationCommand`s and the
/// canonical writer. One multi-record `fox.animation`: the locomotion state machine
/// (`an_fox`: idle ↔ a speed blend of Walk/Run), a motion-matching variant
/// (`an_foxmatch`) and clips binding the model's Survey/Walk/Run animations by name.
fn fox_animations(schema: &SchemaDb) -> Vec<(&'static str, String)> {
    use ara_anim::{
        document::{AnimAsset, AnimationCommand},
        graph::{
            BlendSample, Comparison, Condition, ControllerData, Node, NodeKind, Parameter,
            ParameterKind, State, Transition,
        },
        matching::{MatchConfig, MatchNode},
    };
    let (path, id) = ANIMATION;
    let mut workspace = Workspace::default();
    workspace.insert(path, Document::new("animation", AssetId::parse(id).unwrap()));
    let mut store = ara_ops::Store::new(schema.clone(), workspace);
    let reference = |record: &str| AssetRef(format!("@{path}#{record}"));
    let float = || Parameter {
        kind: ParameterKind::Float,
        ..Default::default()
    };
    let clip = |id: &str| Node {
        kind: NodeKind::Clip,
        clip: Some(reference(id)),
        ..Default::default()
    };
    let speed = |comparison, value: f32| Condition {
        parameter: "speed".into(),
        comparison,
        float_value: value,
        ..Default::default()
    };
    // Idle ↔ move on ground speed, 0.2 s cross-fades (clip timebase 48 kHz).
    let machine = |nodes: BTreeMap<String, Node>, moving: &str, cycle: u32| ControllerData {
        parameters: BTreeMap::from([("speed".into(), float())]),
        initial: "idle".into(),
        fixed_step: 800,
        extract_root_motion: true,
        nodes,
        states: BTreeMap::from([
            (
                "idle".into(),
                State {
                    node: "idle".into(),
                    ..Default::default()
                },
            ),
            (
                "move".into(),
                State {
                    node: moving.into(),
                    duration: cycle,
                    ..Default::default()
                },
            ),
        ]),
        transitions: vec![
            Transition {
                id: "start".into(),
                from: "idle".into(),
                to: "move".into(),
                duration: 9600,
                conditions: vec![speed(Comparison::Greater, 0.15)],
                ..Default::default()
            },
            Transition {
                id: "stop".into(),
                from: "move".into(),
                to: "idle".into(),
                duration: 9600,
                conditions: vec![speed(Comparison::Less, 0.075)],
                ..Default::default()
            },
        ],
    };
    let blend = machine(
        BTreeMap::from([
            ("idle".into(), clip("an_survey")),
            ("walk".into(), clip("an_walk")),
            ("run".into(), clip("an_run")),
            (
                "move".into(),
                Node {
                    kind: NodeKind::Blend1d,
                    parameter_x: "speed".into(),
                    samples: vec![
                        BlendSample {
                            node: "walk".into(),
                            x: 1.4,
                            y: 0.0,
                        },
                        BlendSample {
                            node: "run".into(),
                            x: 4.5,
                            y: 0.0,
                        },
                    ],
                    ..Default::default()
                },
            ),
        ]),
        "move",
        // Mean of the Walk (0.708 s) and Run (1.158 s) cycles.
        44_800,
    );
    let config = MatchConfig {
        clips: vec![reference("an_walk"), reference("an_run")],
        // Hind and front paws: b_LeftFoot02, b_RightFoot02, b_RightHand, b_LeftHand.
        bones: ["node21", "node25", "node11", "node14"]
            .map(String::from)
            .to_vec(),
        horizons: vec![9600, 19200],
        ..Default::default()
    };
    let query: Vec<String> = (0..config.dimensions())
        .map(|i| format!("feature{i:02}"))
        .collect();
    let mut matching = machine(
        BTreeMap::from([
            ("idle".into(), clip("an_survey")),
            (
                "locomotion".into(),
                Node {
                    kind: NodeKind::MotionMatching,
                    clip: Some(reference("an_foxmatch")),
                    matching: Some(MatchNode {
                        query: query.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ),
        ]),
        "locomotion",
        0,
    );
    matching
        .parameters
        .extend(query.into_iter().map(|name| (name, float())));
    let named = |name: &str| AnimAsset {
        name: name.into(),
        ..Default::default()
    };
    let records = [
        (
            "an_fox",
            AnimAsset {
                name: "Fox locomotion".into(),
                controller: Some(blend),
                ..Default::default()
            },
        ),
        (
            "an_foxmatch",
            AnimAsset {
                name: "Fox motion matching".into(),
                controller: Some(matching),
                motion_database: Some(config),
                ..Default::default()
            },
        ),
        ("an_survey", named("Survey")),
        ("an_walk", named("Walk")),
        ("an_run", named("Run")),
    ];
    for (record, asset) in records {
        AnimationCommand::Create(asset)
            .apply(
                &mut store,
                ara_ops::TxnMeta::by(ara_ops::Author::Agent("demo".into())),
                AssetId::parse(id).unwrap(),
                RecordId::parse(record).unwrap(),
            )
            .unwrap();
    }
    let doc = store.workspace().doc(id).unwrap();
    vec![(path, ara_anim::document::write(doc, store.schema()))]
}

#[test]
fn demo_animation_document_is_canonical_and_current() {
    let schema = ara_runtime::schema(&[]).unwrap();
    for (relative, expected) in fox_animations(&schema) {
        let path = demo().join(relative);
        if std::env::var_os("ARA_UPDATE_DEMO").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &expected).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            expected,
            "{relative}: regenerate with ARA_UPDATE_DEMO=1"
        );
    }
}

#[test]
fn motion_matching_controller_drives_the_fox_deterministically() {
    let matched = |seed| {
        runtime_with(seed, |scene| {
            scene.replace(
                "@anim/fox.animation#an_fox",
                "@anim/fox.animation#an_foxmatch",
            )
        })
    };
    let hash = |rt: &Runtime| rt.play().session().state_hash().unwrap();
    let mut a = matched(3);
    let mut b = matched(3);
    for rt in [&mut a, &mut b] {
        rt.step(45, &walk(45)).unwrap();
    }
    let checkpoint = b.play_mut().checkpoint().unwrap();
    for rt in [&mut a, &mut b] {
        rt.step(45, &walk(45)).unwrap();
    }
    assert_eq!(hash(&a), hash(&b), "motion matching is deterministic");
    let world = a.world();
    let slot = world
        .iter_entities()
        .find(|e| e.get::<Identity>().is_some_and(|id| id.0 == "e_fox"))
        .and_then(|e| e.get::<AnimatorSlot>())
        .unwrap();
    assert!(slot.fault.is_none(), "{:?}", slot.fault);
    let predictor = slot.matching.as_ref().unwrap().predictor;
    assert!(
        predictor.motion.value.length() > 0.5,
        "predictor follows the walking fox: {predictor:?}"
    );
    b.play_mut().restore(&checkpoint).unwrap();
    b.step(45, &walk(45)).unwrap();
    assert_eq!(hash(&a), hash(&b), "checkpoint restores predictor state");
}
