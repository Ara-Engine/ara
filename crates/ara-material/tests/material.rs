#![allow(clippy::unwrap_used)] // Test fixtures may panic on violated expectations.
mod common;
use ara_cmd::{Batch, Call, Registry, Session};
use ara_diag::FixAction;
use ara_doc::Workspace;
use ara_graph::{DomainAdapter, Editor, Graph};
use ara_material::*;
use ara_ops::{Author, Store};
use common::{assets, example, root, schema};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};

/// Targets these tests compile. The Mac's slangc cannot load dxc for DXIL
/// (docs/dev/macos.md: Windows is the canonical cook host).
const TARGETS: &[ShaderTarget] = if cfg!(target_os = "macos") {
    &[ShaderTarget::Spirv]
} else {
    &[ShaderTarget::Spirv, ShaderTarget::Dxil]
};

const EXAMPLES: &[&str] = &[
    "painted-metal",
    "car-paint",
    "foliage",
    "emissive-screen",
    "ui-rounded-panel",
    "terrain-blend",
];
fn fixture(path: PathBuf, text: &str) {
    if std::env::var_os("ARA_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; run with ARA_UPDATE_FIXTURES=1", path.display())),
        text,
        "{}",
        path.display()
    );
}

#[test]
fn example_roundtrips_and_codegen_snapshots() {
    let db = schema();
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    let registry = library::registry().unwrap();
    assert!(library::BUILTINS.len() >= 30);
    for name in EXAMPLES {
        let m = example(name);
        check(&m).unwrap();
        let text = dsl::print(&m).unwrap();
        assert_eq!(m, dsl::reconcile(&text, &m).unwrap().material);
        assert!(!text.contains("gr_"));
        assert!(!text.contains("n_"));
        fixture(root().join(format!("examples/{name}.material")), &text);
        let graph = m.to_graph(Editor::default()).unwrap();
        assert!(graph.validate(&registry, &MaterialAdapter).is_empty());
        let json = graph.write(&db).unwrap();
        fixture(root().join(format!("examples/{name}.graph")), &json);
        let back = Graph::read(&json, &db).unwrap();
        assert_eq!(graph, back);
        assert_eq!(m, MaterialAdapter.compile(&back).unwrap());
        assert_eq!(
            text,
            dsl::print(&Material::from_graph(&back).unwrap()).unwrap()
        );
        assert_eq!(
            json,
            Material::from_graph(&back)
                .unwrap()
                .to_graph(Editor::default())
                .unwrap()
                .write(&db)
                .unwrap()
        );
        let generated = generate(&m, &Instance::default()).unwrap();
        fixture(
            root().join(format!("tests/snapshots/{name}.slang")),
            &generated.source,
        );
        for binding in &generated.mappings {
            assert!(
                generated
                    .nodes_at_line(binding.start_line)
                    .any(|id| id == binding.node)
            );
        }
    }
}

#[test]
fn generated_examples_compile_to_spirv_and_dxil() {
    let compiler = ShaderCompiler::discover();
    for name in EXAMPLES {
        let generated = generate(&example(name), &Instance::default()).unwrap();
        for &target in TARGETS {
            let compiled = compiler
                .compile(&generated, target)
                .unwrap_or_else(|e| panic!("{name}/{target:?}: {e}"));
            assert!(compiled.bytes.len() > 100);
            assert!(!compiled.compiler_version.trim().is_empty());
        }
    }
}

#[test]
fn type_fixes_are_real_undoable_commands() {
    let mut db = schema();
    let mut registry = Registry::default();
    commands::register(&mut registry, &mut db);
    assert!(db.verify().is_empty());
    assert!(registry.check_examples(&db).is_empty());
    let mut m = example("emissive-screen");
    let glow = m.node_id("glow").unwrap().to_owned();
    m.bindings.get_mut(&glow).unwrap().ty = Type::Vec3;
    let errors = check(&m).unwrap_err();
    let fix = errors
        .iter()
        .find(|d| d.code == codes::TYPE && d.message.starts_with("declared"))
        .unwrap()
        .fixes[0]
        .clone();
    let FixAction::Command { command, params } = fix.action else {
        panic!("expected command");
    };
    let mut ws = Workspace::new(assets());
    ws.insert(
        "materials/screen.graph",
        m.to_graph(Editor::default())
            .unwrap()
            .to_document()
            .unwrap(),
    );
    let mut store = Store::new(db, ws);
    let mut batch = Batch::new("gr_screen", []);
    batch.calls.push(Call { command, params });
    registry
        .apply(&mut store, &batch, &Session::human())
        .unwrap();
    let live = Graph::from_document(store.workspace().doc("gr_screen").unwrap()).unwrap();
    check(&Material::from_graph(&live).unwrap()).unwrap();
    store.undo_last(&Author::Human).unwrap();
    let live = Graph::from_document(store.workspace().doc("gr_screen").unwrap()).unwrap();
    assert!(check(&Material::from_graph(&live).unwrap()).is_err());
    for text in [
        "material X : unlit budget 1 { let a : ColorLinear = linear(1, 2); output color = a; }",
        "material X : unlit budget 1 { let a : ColorSRGB = srgb(1, 1, 1); output color = a; }",
    ] {
        let errors = check(&dsl::parse(text, "gr_x").unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .filter(|d| d.code == codes::TYPE)
                .all(|d| !d.fixes.is_empty())
        );
    }
}

#[test]
fn variants_costs_domains_and_constant_folding() {
    let m = example("car-paint");
    let base = Instance::default();
    let key = base.variant_key(&m).unwrap();
    let runtime = Instance {
        overrides: BTreeMap::from([(
            m.node_id("roughness").unwrap().into(),
            ParameterValue::Numeric(vec![0.4]),
        )]),
    };
    assert_eq!(key, runtime.variant_key(&m).unwrap());
    let specialized = Instance {
        overrides: BTreeMap::from([(
            m.node_id("coat").unwrap().into(),
            ParameterValue::Bool(false),
        )]),
    };
    assert_ne!(key, specialized.variant_key(&m).unwrap());
    let invalid = Instance {
        overrides: BTreeMap::from([(
            m.node_id("coat").unwrap().into(),
            ParameterValue::Numeric(vec![0.5]),
        )]),
    };
    assert!(invalid.validate(&m).is_err());
    let mut over = m.clone();
    over.variant_budget = 1;
    assert!(
        check(&over)
            .unwrap_err()
            .iter()
            .any(|d| d.code == codes::VARIANT_BUDGET)
    );
    assert_eq!(
        generate(&example("terrain-blend"), &base)
            .unwrap()
            .cost
            .texture_samples,
        6
    );
    assert_eq!(
        generate(&example("foliage"), &base)
            .unwrap()
            .cost
            .texture_samples,
        2
    );
    let folded=dsl::parse("material Fold : surface budget 1 { let a : float = 0.25; let b : float = 0.5; let c : float = add(a, b); output roughness = c; }","gr_fold").unwrap();
    assert_eq!(
        fold_constants(&folded).unwrap().bindings[folded.node_id("c").unwrap()].expr,
        Expr::Literal { values: vec![0.75] }
    );
    assert_eq!(generate(&folded, &base).unwrap().cost, Cost::default());
    let mut forbidden = example("terrain-blend");
    forbidden.domain = Domain::Ui;
    assert!(
        check(&forbidden)
            .unwrap_err()
            .iter()
            .any(|d| d.code == codes::DOMAIN)
    );
}

#[test]
fn saved_instance_overrides_are_typed_assets_and_undoable() {
    let mut db = schema();
    let mut registry = Registry::default();
    commands::register(&mut registry, &mut db);
    let material = example("painted-metal");
    let asset = InstanceAsset {
        definition: ara_schema::AssetRef("@materials/paint.graph#gr_painted".into()),
        overrides: BTreeMap::new(),
    };
    let doc = asset
        .to_document(ara_doc::AssetId::parse("mi_paint").unwrap())
        .unwrap();
    let text = ara_doc::write(&doc, &db);
    let read = ara_doc::read(&text, "materials/paint.materialinstance", &db, None);
    assert!(!ara_diag::has_errors(&read.diags));
    assert_eq!(
        asset,
        InstanceAsset::from_document(&read.doc.unwrap()).unwrap()
    );
    let mut ws = Workspace::new(assets());
    ws.insert(
        "materials/paint.graph",
        material
            .to_graph(Editor::default())
            .unwrap()
            .to_document()
            .unwrap(),
    );
    ws.insert("materials/paint.materialinstance", doc);
    let mut store = Store::new(db, ws);
    let roughness = material.node_id("roughness").unwrap();
    let paint = material.node_id("paint").unwrap();
    registry
        .apply(
            &mut store,
            &Batch::new(
                "mi_paint",
                [(
                    "material.instance.override",
                    json!({"node":roughness,"values":[0.8]}),
                )],
            ),
            &Session::human(),
        )
        .unwrap();
    let live = InstanceAsset::from_document(store.workspace().doc("mi_paint").unwrap()).unwrap();
    assert_eq!(
        live.resolve(&material).unwrap().overrides[roughness],
        ParameterValue::Numeric(vec![0.8])
    );
    let revision = store.revision();
    assert!(
        registry
            .apply(
                &mut store,
                &Batch::new(
                    "mi_paint",
                    [(
                        "material.instance.override",
                        json!({"node":paint,"values":[3]})
                    )]
                ),
                &Session::human()
            )
            .is_err()
    );
    assert_eq!(revision, store.revision());
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        text,
        ara_doc::write(store.workspace().doc("mi_paint").unwrap(), store.schema())
    );
}

#[test]
fn malformed_dsl_and_unknown_graphs_keep_diagnostics() {
    for source in [
        "",
        "material X : surface budget 1 {",
        "material X : surface budget 1 { let a : float = 1e999; }",
        "material X : surface budget 1 { output roughness = missing; }",
        "material X : surface budget 1 { let a : float = 1; let a : float = 2; }",
    ] {
        assert!(dsl::parse(source, "gr_x").is_err());
    }
    let m = example("painted-metal");
    let mut graph = m.to_graph(Editor::default()).unwrap();
    let id = m.node_id("tint").unwrap();
    graph.nodes.get_mut(id).unwrap().kind = "future.operation".into();
    let payload = graph.nodes[id].payload.clone();
    assert!(Material::from_graph(&graph).is_err());
    assert_eq!(payload, graph.nodes[id].payload);
}
