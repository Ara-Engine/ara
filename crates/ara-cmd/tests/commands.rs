//! The registry and starter commands: examples, dry run vs apply, symbols, prefab
//! routing, located errors, scopes and jobs.

use ara_cmd::{Batch, Call, Command, EditCtx, Job, JobCtx, Registry, Session};
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetIndex, IdGen, Workspace, read, write};
use ara_ops::{Author, Store};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

const TYPES: &str = r#"{
  "$schema": "ara://schema/types/1",
  "types": {
    "Light": { "kind": "component", "doc": "A light.", "fields": {
      "kind": { "type": "LightKind", "doc": "Shape." },
      "intensity": { "type": "f32", "doc": "Power.", "unit": "lm", "default": 1000 } } },
    "LightKind": { "kind": "enum", "doc": "Light shape.", "variants": {
      "point": { "doc": "Everywhere." }, "spot": { "doc": "A cone." } } },
    "MeshRenderer": { "kind": "component", "doc": "Draws a mesh.", "fields": {
      "mesh": { "type": "asset", "doc": "Mesh." },
      "tint": { "type": "color", "doc": "Tint." } } }
  }
}"#;

const PREFAB: &str = r#"{
  "$schema": "ara://schema/prefab/1",
  "id": "pf_crate001",
  "records": {
    "e_crt1": { "name": "Crate", "Transform": {} },
    "e_lid1": { "name": "Lid", "parent": "e_crt1", "MeshRenderer": {}, "Transform": { "position": [0, 0.5, 0] } }
  }
}"#;

const SCENE: &str = r#"{ "$schema": "ara://schema/scene/1", "id": "sc_level001", "records": {} }"#;

/// Parameters of the test job.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpawnRowParams {
    /// How many entities to create.
    count: u32,
}

/// Creates a row of entities, reporting progress.
struct SpawnRow;

impl Job for SpawnRow {
    type Params = SpawnRowParams;
    type Output = u32;
    const NAME: &'static str = "test.spawnRow";
    const SUMMARY: &'static str = "Create a row of entities.";

    fn examples() -> Vec<Value> {
        vec![json!({ "count": 2 })]
    }

    fn run(ctx: &mut JobCtx<'_, '_>, p: SpawnRowParams) -> Result<u32, Diagnostic> {
        for i in 0..p.count {
            let id = ctx.edit.new_id(None, "e")?;
            let mut record = serde_json::Map::new();
            record.insert("name".into(), json!(format!("Row {i}")));
            ctx.edit.create(id, record)?;
            ctx.progress((i + 1) as f32 / p.count as f32, "spawned");
        }
        Ok(p.count)
    }
}

/// A command that is never registered, to show `Command` is implementable outside.
#[allow(dead_code)]
struct Noop;

impl Command for Noop {
    type Params = SpawnRowParams;
    type Output = ();
    const NAME: &'static str = "test.noop";
    const SUMMARY: &'static str = "Does nothing.";
    fn examples() -> Vec<Value> {
        Vec::new()
    }
    fn run(_: &mut EditCtx<'_, '_>, _: SpawnRowParams) -> Result<(), Diagnostic> {
        Ok(())
    }
}

fn setup() -> (Store, Registry) {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    assert!(
        db.add_json_types(&ara_json::parse(TYPES).expect("types"))
            .is_empty()
    );
    let mut reg = Registry::builtin(&mut db);
    reg.job::<SpawnRow>(&mut db);
    let problems = db.verify();
    assert!(problems.is_empty(), "{problems:#?}");

    let mut index = AssetIndex::new();
    index.insert("pf_crate001", "prefabs/crate.prefab", Some("prefab".into()));
    index.insert("sc_level001", "levels/level.scene", Some("scene".into()));
    let mut ws = Workspace::new(index.clone());
    for (path, text) in [
        ("prefabs/crate.prefab", PREFAB),
        ("levels/level.scene", SCENE),
    ] {
        let result = read(text, path, &db, Some(&index));
        assert!(result.diags.is_empty(), "{:#?}", result.diags);
        ws.insert(path, result.doc.expect("doc"));
    }
    (Store::with_ids(db, ws, IdGen::seeded(42)), reg)
}

fn scene(store: &Store) -> String {
    write(
        store.workspace().doc("sc_level001").expect("scene"),
        store.schema(),
    )
}

fn agent(scopes: &[&str]) -> Session {
    Session {
        author: Author::Agent("claude".into()),
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
    }
}

fn script() -> Batch {
    let mut b = Batch::new(
        "levels/level.scene",
        [
            (
                "entity.create",
                json!({ "id": "$floor", "name": "Floor", "components": { "Transform": { "scale": [10, 1, 10] } } }),
            ),
            (
                "prefab.instantiate",
                json!({ "prefab": "@prefabs/crate.prefab", "id": "$crate", "parent": "$floor", "position": [1, 0, 2] }),
            ),
            (
                "property.set",
                json!({ "entity": "$crate/e_lid1", "path": "MeshRenderer.tint", "value": [1, 0, 0, 1] }),
            ),
            (
                "component.add",
                json!({ "entity": "$crate", "component": "Light", "value": { "kind": "spot" } }),
            ),
            (
                "transform.set",
                json!({ "entities": ["$floor", "$crate/e_lid1"], "rotation": [0, 90, 0] }),
            ),
            (
                "entity.create",
                json!({ "id": "$lamp", "name": "Lamp", "parent": "$crate/e_lid1" }),
            ),
            (
                "component.add",
                json!({ "entity": "$lamp", "component": "Light" }),
            ),
            (
                "prefab.override.set",
                json!({ "entity": "$crate", "path": "name", "value": "Gold crate" }),
            ),
            (
                "entity.reparent",
                json!({ "entity": "$lamp", "parent": "$floor", "index": 0 }),
            ),
            (
                "component.remove",
                json!({ "entity": "$crate/e_lid1", "component": "MeshRenderer" }),
            ),
            (
                "prefab.override.revert",
                json!({ "entity": "$crate", "path": "Light" }),
            ),
            ("entity.inspect", json!({ "entity": "$crate/e_lid1" })),
        ],
    );
    b.why = Some("Set dressing".into());
    b
}

#[test]
fn every_example_validates_and_deserializes() {
    let (store, reg) = setup();
    let problems = reg.check_examples(store.schema());
    assert!(problems.is_empty(), "{problems:#?}");
    insta::assert_snapshot!(ara_json::to_canonical_string(&reg.catalog()));
}

#[test]
fn dry_run_returns_the_same_diff_as_apply() {
    let (mut store, reg) = setup();
    let before = scene(&store);

    let mut dry = script();
    dry.dry_run = true;
    let preview = reg
        .apply(&mut store, &dry, &Session::human())
        .expect("dry run");
    assert!(preview.dry_run && preview.txn.is_none());
    assert_eq!(scene(&store), before);
    assert_eq!(store.revision(), 0);

    let applied = reg
        .apply(&mut store, &script(), &Session::human())
        .expect("apply");
    assert_eq!(applied.ops, preview.ops);
    assert_eq!(applied.symbols, preview.symbols);
    assert_eq!(applied.outputs, preview.outputs);
    assert_eq!(store.history().len(), 1, "a batch is one transaction");
    assert_eq!(store.history()[0].why.as_deref(), Some("Set dressing"));

    let after = scene(&store);
    insta::assert_snapshot!("scripted_scene", after);
    insta::assert_snapshot!(
        "inspect_output",
        ara_json::to_canonical_string(&applied.outputs[11])
    );

    store.undo_last(&Author::Human).expect("undo the batch");
    assert_eq!(scene(&store), before);
}

#[test]
fn deleting_inside_an_instance_removes_from_it() {
    let (mut store, reg) = setup();
    let first = reg
        .apply(&mut store, &script(), &Session::human())
        .expect("apply");
    let crate_id = &first.symbols["$crate"];
    let floor = &first.symbols["$floor"];
    let batch = Batch::new(
        "sc_level001",
        [(
            "entity.delete",
            json!({ "entities": [format!("{crate_id}/e_lid1")] }),
        )],
    );
    reg.apply(&mut store, &batch, &Session::human())
        .expect("delete inside");
    let rec = store
        .workspace()
        .doc("sc_level001")
        .and_then(|d| d.record(crate_id))
        .cloned()
        .expect("instance");
    assert_eq!(
        rec["removed"],
        json!(["e_lid1"]),
        "the component removal folds into the entity removal"
    );
    assert!(
        !rec["overrides"]
            .as_object()
            .expect("overrides")
            .keys()
            .any(|k| k.starts_with("e_lid1/"))
    );

    let batch = Batch::new(
        "sc_level001",
        [("entity.delete", json!({ "entities": [floor] }))],
    );
    let out = reg
        .apply(&mut store, &batch, &Session::human())
        .expect("delete subtree");
    assert_eq!(out.outputs[0]["deleted"].as_array().map(Vec::len), Some(3));
    assert!(
        store
            .workspace()
            .doc("sc_level001")
            .expect("scene")
            .records()
            .is_empty()
    );
}

#[test]
fn errors_carry_codes_and_call_locations() {
    let (mut store, reg) = setup();
    let fail =
        |store: &mut Store, calls: Vec<(&'static str, Value)>, session: &Session| -> Diagnostic {
            reg.apply(store, &Batch::new("sc_level001", calls), session)
                .expect_err("fails")
                .remove(0)
        };
    let d = fail(
        &mut store,
        vec![("entity.craete", json!({}))],
        &Session::human(),
    );
    assert_eq!(d.code, codes::UNKNOWN_COMMAND);
    assert!(d.message.contains("did you mean `entity.create`"));

    let d = fail(
        &mut store,
        vec![
            ("entity.create", json!({})),
            (
                "transform.set",
                json!({ "entities": [], "position": [1, 2] }),
            ),
        ],
        &Session::human(),
    );
    assert_eq!(
        (d.code, d.location.command, d.location.path.as_deref()),
        (codes::TYPE_MISMATCH, Some(1), Some("params.position"))
    );

    let d = fail(
        &mut store,
        vec![
            ("entity.create", json!({ "id": "$a" })),
            (
                "component.add",
                json!({ "entity": "$b", "component": "Light" }),
            ),
        ],
        &Session::human(),
    );
    assert_eq!(
        (d.code, d.location.command),
        (codes::UNRESOLVED_SYMBOL, Some(1))
    );

    let d = fail(
        &mut store,
        vec![
            (
                "entity.create",
                json!({ "id": "$a", "components": { "Transform": {} } }),
            ),
            (
                "component.add",
                json!({ "entity": "$a", "component": "Transform" }),
            ),
        ],
        &Session::human(),
    );
    assert_eq!(d.code, codes::COMPONENT_EXISTS);

    let d = fail(
        &mut store,
        vec![("entity.create", json!({ "components": { "Lihgt": {} } }))],
        &Session::human(),
    );
    assert_eq!(
        (d.code, d.location.path.as_deref()),
        (codes::UNKNOWN_TYPE, Some("params.components[Lihgt]"))
    );

    let d = fail(
        &mut store,
        vec![("entity.create", json!({}))],
        &agent(&["read"]),
    );
    assert_eq!(d.code, codes::MISSING_SCOPE);
    assert_eq!(store.revision(), 0, "failed batches change nothing");

    reg.apply(
        &mut store,
        &Batch::new("sc_level001", [("entity.create", json!({}))]),
        &agent(&["edit*"]),
    )
    .expect("prefix scope");
}

#[test]
fn jobs_report_progress_and_commit_once() {
    let (mut store, reg) = setup();
    let mut seen = Vec::new();
    let batch = Batch {
        calls: vec![Call {
            command: "test.spawnRow".into(),
            params: json!({ "count": 2 }),
        }],
        ..Batch::new("sc_level001", [])
    };
    let out = reg
        .run(&mut store, &batch, &Session::human(), &mut |f, m| {
            seen.push((f, m.to_owned()))
        })
        .expect("job");
    assert_eq!(out.outputs, [json!(2)]);
    assert_eq!(
        seen,
        [(0.5, "spawned".to_owned()), (1.0, "spawned".to_owned())]
    );
    assert_eq!(store.history().len(), 1);
}

#[test]
fn name_selectors_resolve_and_bare_names_get_fixes() {
    let (mut store, reg) = setup();
    let made = reg
        .apply(
            &mut store,
            &Batch::new(
                "sc_level001",
                [
                    ("entity.create", json!({ "id": "$floor", "name": "Floor" })),
                    (
                        "entity.create",
                        json!({ "id": "$crate", "name": "Crate", "parent": "$floor" }),
                    ),
                    ("entity.create", json!({ "name": "Barrel" })),
                    ("entity.create", json!({ "name": "Barrel" })),
                    (
                        "prefab.instantiate",
                        json!({ "id": "$box", "prefab": "@prefabs/crate.prefab", "name": "Box" }),
                    ),
                ],
            ),
            &Session::human(),
        )
        .expect("setup batch");
    let crate_id = made.symbols["$crate"].clone();
    let lid = format!("{}/e_lid1", made.symbols["$box"]);

    let set = |entity: &str| {
        (
            "property.set",
            json!({ "entity": entity, "path": "name", "value": "Renamed" }),
        )
    };
    for (selector, want) in [
        ("name:Crate", &crate_id),
        ("name:Floor/Crate", &crate_id),
        ("name:Box/Lid", &lid),
    ] {
        let r = reg
            .apply(
                &mut store,
                &Batch::new("sc_level001", [set(selector)]),
                &Session::human(),
            )
            .unwrap_or_else(|d| panic!("{selector}: {d:#?}"));
        assert_eq!(r.outputs[0]["entity"], json!(want), "{selector}");
        store.undo_last(&Author::Human).expect("undo");
    }

    let fail = |store: &mut Store, entity: &str| {
        reg.apply(
            store,
            &Batch::new("sc_level001", [set(entity)]),
            &Session::human(),
        )
        .expect_err("fails")
        .remove(0)
    };
    let d = fail(&mut store, "Crate");
    assert_eq!(
        (d.code, d.location.path.as_deref()),
        (codes::TYPE_MISMATCH, Some("params.entity"))
    );
    assert!(
        d.message
            .contains(&format!("did you mean `{crate_id}` (Crate, under Floor)?")),
        "{}",
        d.message
    );
    assert_eq!(
        serde_json::to_value(&d.fixes[0]).expect("fix"),
        json!({ "title": format!("Use `{crate_id}` (Crate, under Floor)"), "action": "set", "path": "params.entity", "value": crate_id })
    );
    let d = fail(&mut store, "name:Barrel");
    assert_eq!((d.code, d.fixes.len()), (codes::AMBIGUOUS_ENTITY, 2));
    let d = fail(&mut store, "name:Crat");
    assert_eq!(d.code, codes::RECORD_NOT_FOUND);
    assert!(
        d.message.contains("did you mean `name:Crate`?") && d.fixes.len() == 1,
        "{d:#?}"
    );
    let d = fail(&mut store, "e_crt1");
    assert!(
        d.message.contains("it is in `prefabs/crate.prefab`"),
        "{}",
        d.message
    );
    assert_eq!(
        serde_json::to_value(&d.fixes[0].action).expect("fix")["path"],
        json!("doc")
    );
}

#[test]
fn asset_params_may_skip_the_at_and_instances_answer_to_their_root_id() {
    let (mut store, reg) = setup();
    let r = reg
        .apply(
            &mut store,
            &Batch::new(
                "sc_level001",
                [
                    (
                        "prefab.instantiate",
                        json!({ "id": "$a", "prefab": "prefabs/crate.prefab" }),
                    ),
                    (
                        "prefab.instantiate",
                        json!({ "id": "$b", "prefab": "pf_crate001" }),
                    ),
                    // The prefab's root record, addressed through the instance: the instance.
                    (
                        "property.set",
                        json!({ "entity": "$a/e_crt1", "path": "name", "value": "Box" }),
                    ),
                ],
            ),
            &Session::human(),
        )
        .unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(r.outputs[2]["entity"], json!(r.symbols["$a"]));
    let record = store
        .workspace()
        .doc("sc_level001")
        .and_then(|d| d.record(&r.symbols["$b"]))
        .cloned()
        .expect("instance");
    assert_eq!(record["prefab"], "@prefabs/crate.prefab#pf_crate001");
}
