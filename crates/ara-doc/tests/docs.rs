//! Canonical files, reference repair, prefab resolution and structural checks.

mod common;

use ara_diag::codes;
use ara_doc::{AssetId, read, write};
use serde_json::{Map, Value, json};

#[test]
fn canonical_fixtures_round_trip_byte_identical() {
    let db = common::db();
    let ix = common::index(&db);
    for rel in common::DOCS {
        let doc = common::load(rel, &db, &ix);
        assert_eq!(
            write(&doc, &db),
            common::text(rel),
            "{rel} is not canonical"
        );
    }
}

#[test]
fn reading_normalizes_renames_and_repairs() {
    let db = common::db();
    let ix = common::index(&db);
    let result = read(
        &common::text("input/messy.scene"),
        "input/messy.scene",
        &db,
        Some(&ix),
    );
    let notes: Vec<(String, String)> = result
        .diags
        .iter()
        .map(|d| (d.code.to_string(), format!("{}", d.location)))
        .collect();
    assert_eq!(
        notes,
        [
            (
                "ARA1108".into(),
                "input/messy.scene · e_b2 · Renderer".into()
            ),
            (
                "ARA1303".into(),
                "input/messy.scene · e_b2 · MeshRenderer.mesh".into()
            ),
            ("ARA1304".into(), "input/messy.scene · e_a1 · prefab".into()),
        ]
    );
    let doc = result.doc.expect("document");
    let text = write(&doc, &db);
    insta::assert_snapshot!(text);
    let again = read(&text, "x", &db, Some(&ix));
    assert!(again.diags.is_empty(), "{:#?}", again.diags);
}

#[test]
fn syntax_errors_carry_positions() {
    let db = common::db();
    let result = read(
        "{\n  \"$schema\": \"ara://schema/scene/1\",\n  \"id\": 1,\n}",
        "a.scene",
        &db,
        None,
    );
    assert!(result.doc.is_none());
    let d = &result.diags[0];
    assert_eq!(
        (d.code, d.location.line, d.location.column),
        (codes::JSON_SYNTAX, Some(4), Some(1))
    );

    let result = read(
        r#"{"$schema": "ara://schema/scene/9", "id": "sc_1", "records": {}}"#,
        "a.scene",
        &db,
        None,
    );
    assert_eq!(result.diags[0].code, codes::NEWER_VERSION);
}

#[test]
fn prefab_instances_resolve_with_overrides_removals_and_nesting() {
    let db = common::db();
    let ws = common::workspace(&db);
    let mut out = Vec::new();
    let resolved = ws.resolve(&AssetId::parse("sc_arena001").expect("id"), &db, &mut out);
    assert!(out.is_empty(), "{out:#?}");
    let view: Map<String, Value> = resolved
        .iter()
        .map(|(addr, e)| {
            let mut v = e.record.clone();
            if let Some(p) = &e.parent {
                v.insert("parent".into(), json!(p));
            }
            (addr.clone(), Value::Object(v))
        })
        .collect();
    insta::assert_snapshot!(ara_json::to_canonical_string(&Value::Object(view)));
}

#[test]
fn bad_overrides_and_removals_are_reported() {
    let db = common::db();
    let mut ws = common::workspace(&db);
    let scene = r#"{
      "$schema": "ara://schema/scene/1",
      "id": "sc_bad00001",
      "records": {
        "e_i1": {
          "prefab": "@prefabs/crate.prefab#pf_crate001",
          "overrides": {
            "e_nope/Transform.position": [1, 2, 3],
            "Transform.positon": [1, 2, 3],
            "e_lid1/Light.intensity": 5,
            "e_lid1/MeshRenderer.tint": "red",
            "parent": "e_x"
          },
          "removed": ["e_ghost"]
        }
      }
    }"#;
    let doc = read(scene, "bad.scene", &db, Some(ws.index()))
        .doc
        .expect("document");
    let id = doc.id.clone();
    ws.insert("bad.scene", doc);
    let found: Vec<(String, String)> = ws
        .check(&id, None, &db)
        .iter()
        .map(|d| {
            (
                d.code.to_string(),
                d.location.path.clone().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("ARA1405".into(), "removed[#0]".into()),
            ("ARA1111".into(), "overrides[Transform.positon]".into()),
            ("ARA1403".into(), "overrides[e_lid1/Light.intensity]".into()),
            (
                "ARA1102".into(),
                "overrides[e_lid1/MeshRenderer.tint]".into()
            ),
            (
                "ARA1403".into(),
                "overrides[e_nope/Transform.position]".into()
            ),
            ("ARA1404".into(), "overrides[parent]".into()),
        ]
    );
}

#[test]
fn structure_problems_are_reported() {
    let db = common::db();
    let check = |text: &str| -> Vec<String> {
        read(text, "t", &db, None)
            .diags
            .iter()
            .map(|d| d.code.to_string())
            .collect()
    };
    let cycle = r#"{"$schema": "ara://schema/scene/1", "id": "sc_1", "records": {
        "e_a": {"parent": "e_b"}, "e_b": {"parent": "e_a"}, "e_c": {"parent": "e_zz"}, "e_d": {"parent": "e_c/e_x"}}}"#;
    assert_eq!(check(cycle), ["ARA1205", "ARA1205", "ARA1206", "ARA1206"]);

    let two_roots =
        r#"{"$schema": "ara://schema/prefab/1", "id": "pf_1", "records": {"e_a": {}, "e_b": {}}}"#;
    assert_eq!(check(two_roots), ["ARA1208"]);

    let misc = r#"{"$schema": "ara://schema/scene/1", "id": "sc_1", "extra": 1, "records": {
        "e_a": {"order": "a10", "prefab": "@prefabs/x.prefab#pf_1", "Transform": {}},
        "x_b": {}, "e_c": {"overrides": {}}, "e_d": {"Transfrom": {}}}}"#;
    assert_eq!(
        check(misc),
        [
            "ARA1101", "ARA1207", "ARA1211", "ARA1202", "ARA1406", "ARA1106"
        ]
    );
}

#[test]
fn dangling_entity_references_warn_with_a_fix() {
    let db = common::db();
    let scene = r#"{"$schema": "ara://schema/scene/1", "id": "sc_1", "records": {
        "e_a": {"Follow": {"target": "e_b"}}, "e_b": {}, "e_c": {"Follow": {"target": "e_gone"}}}}"#;
    let diags = read(scene, "t", &db, None).diags;
    assert_eq!(diags.len(), 1, "{diags:#?}");
    let d = &diags[0];
    assert_eq!(
        (
            d.code,
            d.location.record.as_deref(),
            d.location.path.as_deref()
        ),
        (
            codes::DANGLING_ENTITY_REF,
            Some("e_c"),
            Some("Follow.target")
        )
    );
    assert!(!d.is_error(), "dangling references don't block edits");
}

#[test]
fn local_edits_are_sets_of_a_records_own_data() {
    use ara_doc::{Edit, RecordId};
    let doc = AssetId::parse("sc_local001").expect("id");
    let id = RecordId::parse("e_a").expect("record");
    let set = |path: &str| Edit::Set {
        doc: doc.clone(),
        id: id.clone(),
        path: path.parse().expect("path"),
        value: Some(json!(1)),
    };
    for path in ["name", "Transform", "Transform.position", "overrides"] {
        assert!(set(path).is_local(), "{path}");
    }
    for path in ["parent", "order", "prefab", "removed"] {
        assert!(!set(path).is_local(), "{path}");
    }
    let structural = [
        Edit::Create {
            doc: doc.clone(),
            id: id.clone(),
            record: Map::new(),
        },
        Edit::Delete {
            doc: doc.clone(),
            id: id.clone(),
        },
        Edit::Move {
            doc: doc.clone(),
            id: id.clone(),
            parent: None,
            order: None,
        },
        Edit::Document {
            doc: doc.clone(),
            path: "levels/local.scene".into(),
            kind: Some("scene".into()),
        },
    ];
    assert!(structural.iter().all(|e| !e.is_local()));
}

#[test]
fn local_checks_report_what_the_whole_check_reports_for_those_records() {
    use std::collections::BTreeSet;
    let db = common::db();
    let mut ws = common::workspace(&db);
    // A bad instance, a plain record carrying instance-only fields, a valid record,
    // and a structural problem on a record the local check leaves out.
    let scene = r#"{
      "$schema": "ara://schema/scene/1",
      "id": "sc_local001",
      "records": {
        "e_i1": { "prefab": "@prefabs/crate.prefab#pf_crate001", "overrides": { "Transform.positon": [1, 2, 3] }, "removed": ["e_ghost"] },
        "e_p1": { "name": "Plain", "overrides": {} },
        "e_ok": { "name": "Fine", "Transform": {} },
        "e_or": { "parent": "e_gone" }
      }
    }"#;
    let doc = read(scene, "local.scene", &db, Some(ws.index()))
        .doc
        .expect("document");
    let id = doc.id.clone();
    ws.insert("local.scene", doc);
    let records: BTreeSet<_> = ["e_i1", "e_p1", "e_ok"]
        .into_iter()
        .map(|r| ara_doc::RecordId::parse(r).expect("record"))
        .collect();
    let key = |d: &ara_diag::Diagnostic| {
        (
            d.code.to_string(),
            d.location.record.clone().unwrap_or_default(),
            d.location.path.clone().unwrap_or_default(),
        )
    };
    let local: Vec<_> = ws.check_local(&id, &records, &db).iter().map(key).collect();
    let whole: Vec<_> = ws
        .check(&id, None, &db)
        .iter()
        .map(key)
        .filter(|(_, record, _)| records.iter().any(|r| r.as_str() == record))
        .collect();
    assert_eq!(local, whole);
    assert_eq!(
        local
            .iter()
            .map(|(code, record, _)| (code.as_str(), record.as_str()))
            .collect::<Vec<_>>(),
        [
            ("ARA1406", "e_p1"),
            ("ARA1405", "e_i1"),
            ("ARA1111", "e_i1")
        ]
    );
    // The missing parent is structural: only the whole check reports it.
    assert!(
        ws.check(&id, None, &db)
            .iter()
            .any(|d| d.code == codes::MISSING_PARENT)
    );
}
