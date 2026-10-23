#![allow(clippy::unwrap_used)] // Test fixtures may panic on violated expectations.

use std::{collections::BTreeMap, path::PathBuf};

use ara_cmd::{Batch, Call, Registry, Session};
use ara_diag::FixAction;
use ara_doc::Workspace;
use ara_graph::{DomainAdapter, Editor, Graph};
use ara_material::*;
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

const EXAMPLES: &[&str] = &[
    "painted-metal",
    "car-paint",
    "foliage",
    "emissive-screen",
    "ui-rounded-panel",
    "terrain-blend",
];
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    register(&mut db).unwrap();
    db
}

#[test]
fn saved_instance_overrides_are_validated_and_undoable() {
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
    let mut ws = Workspace::default();
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
    let key = Instance::default().variant_key(&material).unwrap();
    registry
        .apply(
            &mut store,
            &Batch::new(
                "mi_paint",
                [(
                    "material.instance.override",
                    json!({"node":"n_rough","values":[0.8]}),
                )],
            ),
            &Session::human(),
        )
        .unwrap();
    let live = InstanceAsset::from_document(store.workspace().doc("mi_paint").unwrap()).unwrap();
    let instance = live.resolve(&material).unwrap();
    assert_eq!(instance.overrides["n_rough"], [0.8]);
    assert_eq!(key, instance.variant_key(&material).unwrap());
    let revision = store.revision();
    assert!(
        registry
            .apply(
                &mut store,
                &Batch::new(
                    "mi_paint",
                    [(
                        "material.instance.override",
                        json!({"node":"n_paint","values":[16]})
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
fn example(name: &str) -> Material {
    dsl::parse(&std::fs::read_to_string(root().join(format!("examples/{name}.material"))).unwrap())
        .unwrap()
}
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
        assert_eq!(m, dsl::parse(&text).unwrap());
        fixture(root().join(format!("examples/{name}.material")), &text);
        let graph = m.to_graph(Editor::default()).unwrap();
        assert!(graph.validate(&registry, &MaterialAdapter).is_empty());
        let json = graph.write(&db).unwrap();
        fixture(root().join(format!("examples/{name}.graph")), &json);
        let back = Graph::read(&json, &db).unwrap();
        assert_eq!(graph, back);
        assert_eq!(m, MaterialAdapter.compile(&back).unwrap());
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
        let m = example(name);
        let generated = generate(&m, &Instance::default()).unwrap();
        for target in [ShaderTarget::Spirv, ShaderTarget::Dxil] {
            let compiled = compiler
                .compile(&generated, target)
                .unwrap_or_else(|e| panic!("{name}/{target:?}: {e}"));
            assert!(!compiled.compiler_version.trim().is_empty());
            assert!(compiled.bytes.len() > 100);
        }
    }
}

#[test]
fn text_edits_preserve_layout_and_graph_edits_are_the_same_ast() {
    let mut db = schema();
    let mut registry = Registry::default();
    commands::register(&mut registry, &mut db);
    ara_graph::commands::register(&mut registry, &mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(registry.check_examples(&db).is_empty());
    let m = example("painted-metal");
    let mut graph = m.to_graph(Editor::default()).unwrap();
    graph
        .editor
        .positions
        .insert("n_rough".into(), [24.0, 60.0]);
    let before = graph.write(&db).unwrap();
    let mut ws = Workspace::default();
    ws.insert("materials/paint.graph", graph.to_document().unwrap());
    let mut store = Store::new(db, ws);
    let mut edited = m.clone();
    edited.parameters.get_mut("n_rough").unwrap().default = vec![0.6];
    edited.parameters.get_mut("n_rough").unwrap().name = "coatRoughness".into();
    let apply = |store: &mut Store, command: &'static str, params: Value| {
        registry.apply(
            store,
            &Batch::new("gr_painted", [(command, params)]),
            &Session::human(),
        )
    };
    let result = apply(
        &mut store,
        "material.text.apply",
        json!({"text":dsl::print(&edited).unwrap()}),
    )
    .unwrap();
    assert!(!result.ops.is_empty());
    let live = Graph::from_document(store.workspace().doc("gr_painted").unwrap()).unwrap();
    assert_eq!(live.editor, graph.editor);
    assert_eq!(Material::from_graph(&live).unwrap(), edited);
    apply(
        &mut store,
        "graph.param.set",
        json!({"node":"n_rough","name":"value","value":[0.7]}),
    )
    .unwrap();
    let live = Graph::from_document(store.workspace().doc("gr_painted").unwrap()).unwrap();
    let back = Material::from_graph(&live).unwrap();
    assert_eq!(back.parameters["n_rough"].default, [0.7]);
    assert_eq!(back, dsl::parse(&dsl::print(&back).unwrap()).unwrap());
    store.undo_last(&Author::Human).unwrap();
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("gr_painted").unwrap(), store.schema())
    );
}

#[test]
fn type_diagnostics_fixes_apply_as_real_undoable_commands() {
    let mut db = schema();
    let mut registry = Registry::default();
    commands::register(&mut registry, &mut db);
    let mut m = example("emissive-screen");
    m.bindings.get_mut("n_glow").unwrap().ty = Type::Vec3;
    let errors = check(&m).unwrap_err();
    let fix = errors
        .iter()
        .find(|d| d.code == codes::TYPE && d.message.starts_with("declared"))
        .unwrap()
        .fixes[0]
        .clone();
    let FixAction::Command { command, params } = fix.action else {
        panic!("expected typed fix command");
    };
    let graph = m.to_graph(Editor::default()).unwrap();
    let mut ws = Workspace::default();
    ws.insert("materials/screen.graph", graph.to_document().unwrap());
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
        "material gr_x X : unlit budget 1 { let n_a a : float = unknown(); output color = a; }",
        "material gr_x X : unlit budget 1 { let n_a a : ColorLinear = [1, 2]; output color = a; }",
        "material gr_x X : unlit budget 1 { let n_a a : ColorSRGB = [1, 1, 1]; output color = a; }",
    ] {
        let errors = check(&dsl::parse(text).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .filter(|d| d.code == codes::TYPE)
                .all(|d| !d.fixes.is_empty())
        );
    }
}

#[test]
fn variants_costs_domain_restrictions_and_constant_folding() {
    let m = example("car-paint");
    let base = Instance::default();
    let key = base.variant_key(&m).unwrap();
    let runtime = Instance {
        overrides: BTreeMap::from([("n_rough".into(), vec![0.4])]),
    };
    assert_eq!(key, runtime.variant_key(&m).unwrap());
    let specialized = Instance {
        overrides: BTreeMap::from([("n_coat".into(), vec![0.0])]),
    };
    assert_ne!(key, specialized.variant_key(&m).unwrap());
    let invalid = Instance {
        overrides: BTreeMap::from([("n_coat".into(), vec![0.5])]),
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
    let folded=dsl::parse("material gr_fold Fold : surface budget 1 { let n_a a : float = [0.25]; let n_b b : float = [0.5]; let n_c c : float = add(a, b); output roughness = c; }").unwrap();
    assert_eq!(
        fold_constants(&folded).unwrap().bindings["n_c"].expr,
        Expr::Literal { values: vec![0.75] }
    );
    let generated = generate(&folded, &base).unwrap();
    assert_eq!(generated.cost, Cost::default());
    assert!(!generated.source.contains("float n_a ="));
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
fn malformed_dsl_and_graphs_are_rejected_without_losing_data() {
    for text in [
        "",
        "material gr_x X : surface budget 1 {",
        "material gr_x X : surface budget 1 { let n_a a : float = [1e999]; }",
        "material gr_x X : surface budget 1 { output roughness = missing; }",
        "material gr_x X : surface budget 1 { let n_a a : float = [1]; let n_a b : float = [2]; }",
    ] {
        assert!(dsl::parse(text).is_err(), "{text}");
    }
    let m = example("painted-metal");
    let mut g = m.to_graph(Editor::default()).unwrap();
    g.nodes.get_mut("n_color").unwrap().kind = "future.operation".into();
    let payload = g.nodes["n_color"].payload.clone();
    assert!(Material::from_graph(&g).is_err());
    assert_eq!(payload, g.nodes["n_color"].payload);
}
