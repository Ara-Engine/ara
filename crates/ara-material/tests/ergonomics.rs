#![allow(clippy::unwrap_used)] // Test fixtures may panic on violated expectations.
mod common;
use ara_cmd::{Batch, Call, Registry, Session};
use ara_diag::FixAction;
use ara_doc::Workspace;
use ara_graph::{Editor, Graph};
use ara_material::*;
use ara_ops::{Author, Store};
use common::{assets, example, schema};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn store(m: &Material) -> (Store, Registry) {
    let mut db = schema();
    let mut registry = Registry::default();
    commands::register(&mut registry, &mut db);
    ara_graph::commands::register(&mut registry, &mut db);
    let mut graph = m.to_graph(Editor::default()).unwrap();
    let id = m.node_id("roughness").unwrap();
    graph.editor.positions.insert(id.into(), [12.0, 80.0]);
    let mut ws = Workspace::new(assets());
    ws.insert("materials/paint.graph", graph.to_document().unwrap());
    (Store::new(db, ws), registry)
}
fn live(store: &Store) -> Graph {
    Graph::from_document(store.workspace().doc("gr_painted").unwrap()).unwrap()
}
fn apply(
    store: &mut Store,
    registry: &Registry,
    command: &'static str,
    params: Value,
) -> ara_cmd::BatchResult {
    registry
        .apply(
            store,
            &Batch::new("gr_painted", [(command, params)]),
            &Session::human(),
        )
        .unwrap()
}
fn fix(store: &mut Store, registry: &Registry, fix: ara_diag::Fix) {
    let FixAction::Command { command, params } = fix.action else {
        panic!("expected command fix");
    };
    let mut batch = Batch::new("gr_painted", []);
    batch.calls.push(Call { command, params });
    registry.apply(store, &batch, &Session::human()).unwrap();
}

#[test]
fn text_matches_names_rename_preserves_ids_and_plain_rename_has_undoable_recovery() {
    let m = example("painted-metal");
    let id = m.node_id("roughness").unwrap().to_owned();
    let (mut store, registry) = store(&m);
    let initial = live(&store).write(store.schema()).unwrap();
    let mut edited = m.clone();
    edited.parameters.get_mut(&id).unwrap().default = ParameterValue::Numeric(vec![0.6]);
    apply(
        &mut store,
        &registry,
        "material.text.apply",
        json!({"text":dsl::print(&edited).unwrap()}),
    );
    assert_eq!(live(&store).editor.positions[&id], [12.0, 80.0]);
    assert_eq!(Material::from_graph(&live(&store)).unwrap(), edited);
    apply(
        &mut store,
        &registry,
        "material.rename",
        json!({"from":"roughness","to":"coatRoughness"}),
    );
    let renamed = Material::from_graph(&live(&store)).unwrap();
    assert_eq!(renamed.node_id("coatRoughness"), Some(id.as_str()));
    assert_eq!(renamed.outputs["roughness"], id);
    assert_eq!(
        renamed,
        dsl::reconcile(&dsl::print(&renamed).unwrap(), &renamed)
            .unwrap()
            .material
    );
    store.undo_last(&Author::Human).unwrap();
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(initial, live(&store).write(store.schema()).unwrap());
    let source = dsl::print(&m)
        .unwrap()
        .replace("roughness :", "finish :")
        .replace("= roughness;", "= finish;");
    let parsed = dsl::reconcile(&source, &m).unwrap();
    let new_id = parsed.material.node_id("finish").unwrap().to_owned();
    assert_ne!(id, new_id);
    assert!(parsed.material.node_type(&id).is_none());
    assert_eq!(parsed.diagnostics[0].code, codes::PLAIN_RENAME);
    let result = apply(
        &mut store,
        &registry,
        "material.text.apply",
        json!({"text":source}),
    );
    assert!(
        result
            .ops
            .iter()
            .any(|op| matches!(op,ara_doc::Op::Delete{id:deleted,..} if deleted.as_str()==id))
    );
    let diagnostics: Vec<ara_diag::Diagnostic> =
        serde_json::from_value(result.outputs[0]["diagnostics"].clone()).unwrap();
    fix(
        &mut store,
        &registry,
        diagnostics
            .iter()
            .find(|d| d.code == codes::PLAIN_RENAME)
            .unwrap()
            .fixes[0]
            .clone(),
    );
    let recovered = Material::from_graph(&live(&store)).unwrap();
    assert_eq!(recovered.node_id("finish"), Some(id.as_str()));
    assert_eq!(live(&store).editor.positions[&id], [12.0, 80.0]);
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        Material::from_graph(&live(&store))
            .unwrap()
            .node_id("finish"),
        Some(new_id.as_str())
    );
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(initial, live(&store).write(store.schema()).unwrap());
}

#[test]
fn printer_preserves_declarations_and_uses_source_stable_dependency_order() {
    let source = "material Order : surface budget 1 { param z : float = 1; param a : float = 2; let early : float = add(late, z); let independent : float = 3; let late : float = a; output roughness = early; }";
    let m = dsl::parse(source, "gr_order").unwrap();
    let printed = dsl::print(&m).unwrap();
    assert!(printed.find("param z").unwrap() < printed.find("param a").unwrap());
    assert!(printed.find("let independent").unwrap() < printed.find("let late").unwrap());
    assert!(printed.find("let late").unwrap() < printed.find("let early").unwrap());
    assert_eq!(m, dsl::parse(&printed, &m.id).unwrap());
    let already_valid = "material Order : surface budget 1 { param z : float = 1; param a : float = 2; let beta : float = add(z, a); let alpha : float = add(a, z); output roughness = beta; }";
    let printed = dsl::print(&dsl::parse(already_valid, "gr_order").unwrap()).unwrap();
    assert!(printed.find("let beta").unwrap() < printed.find("let alpha").unwrap());
    let surface = example("car-paint");
    let text = dsl::print(&surface).unwrap();
    let fields = surface.domain.output_order();
    let positions: Vec<_> = fields
        .iter()
        .filter_map(|f| text.find(&format!("output {f} =")))
        .collect();
    assert!(positions.windows(2).all(|p| p[0] < p[1]));
}

#[test]
fn typed_literals_hex_annotations_and_static_switches_roundtrip() {
    let source = "material Literals : surface budget 2 { static param coat : bool = true; param tint : ColorSRGB = #0f1066; param color : ColorLinear = linear(0.02, 0.06, 0.4); param normal : TextureNormal = flat_normal; let colorValue #n_kept : ColorLinear = linear(tint); let coverage : float = float(coat); output baseColor = colorValue; output clearCoat = coverage; }";
    let m = dsl::parse(source, "gr_literals").unwrap();
    check(&m).unwrap();
    assert_eq!(m.node_id("colorValue"), Some("n_kept"));
    assert_eq!(
        m.parameters[m.node_id("tint").unwrap()].default,
        ParameterValue::Numeric(vec![15.0 / 255.0, 16.0 / 255.0, 102.0 / 255.0])
    );
    let text = dsl::print(&m).unwrap();
    assert!(!text.contains("n_kept"));
    assert!(text.contains("static param coat : bool = true"));
    assert!(text.contains("srgb("));
    let graph = m.to_graph(Editor::default()).unwrap();
    let back = Material::from_graph(&graph).unwrap();
    assert_eq!(m, back);
    assert_eq!(m, dsl::reconcile(&text, &back).unwrap().material);
    let wrong = dsl::parse(
        "material X : unlit budget 1 { param tint : ColorLinear = #0f1066; output color = tint; }",
        "gr_x",
    )
    .unwrap_err();
    assert_eq!(wrong[0].code, codes::TYPE);
    assert!(!wrong[0].fixes.is_empty());
}

#[test]
fn texture_paths_repair_on_save_and_only_the_renderer_assigns_slots() {
    let source = "material PaintedMetal : surface budget 1 { param paint : TextureColor = @textures/paint.png; let uv0 : vec2 = uv(); let tint : ColorLinear = sampleColor(paint, uv0); output baseColor = tint; }";
    let m = example("painted-metal");
    let (mut store, registry) = store(&m);
    apply(
        &mut store,
        &registry,
        "material.text.apply",
        json!({"text":source}),
    );
    let saved = Material::from_graph(&live(&store)).unwrap();
    let paint = saved.node_id("paint").unwrap();
    assert!(
        dsl::print(&saved)
            .unwrap()
            .contains("@textures/paint.png#tx_paint001")
    );
    let mut moved = assets();
    moved.insert("tx_paint001", "textures/moved-paint.png", None);
    let mut repaired = saved.clone();
    repaired.resolve_assets(&moved).unwrap();
    assert!(
        dsl::print(&repaired)
            .unwrap()
            .contains("@textures/moved-paint.png#tx_paint001")
    );
    let mut calls = Vec::new();
    let upload = Instance::default()
        .pack(&saved, |resource, ty| {
            calls.push((resource.clone(), ty));
            Ok(7)
        })
        .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, Type::TextureColor);
    assert_eq!(upload.rows[0][0], 7.0);
    let serialized = live(&store).write(store.schema()).unwrap();
    assert!(serialized.contains("tx_paint001"));
    assert!(!serialized.contains("7.0"));
    let overrides = Instance {
        overrides: BTreeMap::from([(paint.into(), ParameterValue::Numeric(vec![7.0]))]),
    };
    assert!(overrides.validate(&saved).is_err());
    let other = Instance {
        overrides: BTreeMap::from([(
            paint.into(),
            ParameterValue::Texture(TextureSource::Default(TextureDefault::Black)),
        )]),
    };
    assert_eq!(
        Instance::default().variant_key(&saved).unwrap(),
        other.variant_key(&saved).unwrap()
    );
    assert!(Instance::default().pack(&saved, |_, _| Ok(16)).is_err());
}

#[test]
fn every_draft_pattern_has_an_executable_canonical_source_fix() {
    let m = example("painted-metal");
    let (mut store, registry) = store(&m);
    let cases = [
        "material gr_old Draft : surface budget 1 { param n_a roughness : float = [0.35]; output roughness = roughness; }",
        "material Draft : surface budget 1 { param paint : TextureColor = 7; let uv0 : vec2 = uv(); let color : ColorLinear = sampleColor(paint, uv0); output baseColor = color; }",
        "material Draft : surface budget 1 { let paint : TextureColor = [7]; let uv0 : vec2 = uv(); let color : ColorLinear = sampleColor(paint, uv0); output baseColor = color; }",
        "material Draft : surface budget 1 { let tint : ColorLinear = [0.02, 0.06, 0.4]; output baseColor = tint; }",
        "material Draft : surface budget 2 { param coat : float = [1] variants [[0], [1]]; custom coverage(value: float) -> float cost [0, 2, 0] doc \"Coverage\" slang \"return saturate(value);\"; let clear : float = custom.coverage(coat); output clearCoat = clear; }",
    ];
    for source in cases {
        let errors = dsl::parse(source, "gr_painted").unwrap_err();
        assert!(
            errors
                .iter()
                .all(|d| d.code == codes::LEGACY_SYNTAX && !d.fixes.is_empty())
        );
        for diagnostic in &errors {
            let FixAction::Command { params, .. } = &diagnostic.fixes[0].action else {
                panic!("source command");
            };
            let corrected = params["text"].as_str().unwrap();
            let fixed = dsl::parse(corrected, "gr_painted").unwrap();
            check(&fixed).unwrap();
            assert!(!corrected.contains("n_a"));
            assert!(!corrected.contains("[0.35]"));
        }
        fix(&mut store, &registry, errors[0].fixes[0].clone());
        check(&Material::from_graph(&live(&store)).unwrap()).unwrap();
        store.undo_last(&Author::Human).unwrap();
    }
}

#[test]
fn multiple_renames_are_explicit_choices_and_missing_asset_fixes_are_executable() {
    let previous=dsl::parse("material Pair : surface budget 1 { param a : float = 0.1; param b : float = 0.2; output roughness = a; }","gr_pair").unwrap();
    let edited=dsl::reconcile("material Pair : surface budget 1 { param x : float = 0.1; param y : float = 0.2; output roughness = x; }",&previous).unwrap();
    assert_eq!(edited.diagnostics.len(), 2);
    assert!(
        edited
            .diagnostics
            .iter()
            .all(|d| d.code == codes::PLAIN_RENAME && d.fixes.len() == 2)
    );
    let m = example("painted-metal");
    let (mut store, registry) = store(&m);
    let revision = store.revision();
    let source = "material Missing : surface budget 1 { param newTexture : TextureColor = @textures/missing.png; let uv0 : vec2 = uv(); let tint : ColorLinear = sampleColor(newTexture, uv0); output baseColor = tint; }";
    let errors = registry
        .apply(
            &mut store,
            &Batch::new(
                "gr_painted",
                [("material.text.apply", json!({"text":source}))],
            ),
            &Session::human(),
        )
        .unwrap_err();
    assert_eq!(revision, store.revision());
    fix(&mut store, &registry, errors[0].fixes[0].clone());
    check(&Material::from_graph(&live(&store)).unwrap()).unwrap();
    for name in ["true", "white", "param"] {
        assert!(dsl::parse(&format!("material Reserved : surface budget 1 {{ param {name} : float = 1; output roughness = {name}; }}"),"gr_reserved").is_err());
    }
}

#[test]
fn new_source_reconciles_draft_sidecars_without_changing_ids_or_layout() {
    let m = example("painted-metal");
    let (mut store, registry) = store(&m);
    let mut legacy = live(&store);
    for node in legacy.nodes.values_mut() {
        if let Some(payload) = node.payload.as_object_mut() {
            payload.shift_remove("order");
        }
    }
    let paint = m.node_id("paint").unwrap();
    legacy.nodes.get_mut(paint).unwrap().params =
        BTreeMap::from([("value".into(), json!([7])), ("variants".into(), json!([]))]);
    assert!(Material::from_graph(&legacy).is_err());
    let before = legacy.write(store.schema()).unwrap();
    let editor = legacy.editor.clone();
    let mut ws = Workspace::new(assets());
    ws.insert("materials/paint.graph", legacy.to_document().unwrap());
    store = Store::new(store.schema().clone(), ws);
    apply(
        &mut store,
        &registry,
        "material.text.apply",
        json!({"text":dsl::print(&m).unwrap()}),
    );
    assert_eq!(live(&store).editor, editor);
    assert_eq!(Material::from_graph(&live(&store)).unwrap(), m);
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(live(&store).write(store.schema()).unwrap(), before);
}
