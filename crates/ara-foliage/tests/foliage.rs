#![allow(clippy::unwrap_used)]
use ara_foliage::{Bounds, Exclusion, Rule, Species, SurfaceSample, lod_visible, scatter};
use ara_schema::AssetRef;
fn rule() -> Rule {
    Rule {
        name: "Grass".into(),
        species: Species::Grass,
        asset: AssetRef("@grass.mesh#me_grass001".into()),
        seed: 42,
        cell_size: 2.,
        density: 0.2,
        height: [0., 100.],
        slope: [0., 40.],
        channel: 0,
        min_weight: 0.1,
        noise: 0.3,
        scale: [0.8, 1.2],
        enabled: true,
    }
}
fn surface(_: [f32; 2]) -> Option<SurfaceSample> {
    Some(SurfaceSample {
        height: 10.,
        normal: [0., 1., 0.],
        weights: [1., 0., 0., 0.],
    })
}
#[test]
fn scatter_is_order_independent_and_split_tiles_match_whole() {
    let r = rule();
    let full = Bounds {
        min: [-32., -16.],
        max: [32., 16.],
    };
    let a = Bounds {
        max: [0., 16.],
        ..full
    };
    let b = Bounds {
        min: [0., -16.],
        ..full
    };
    let mut all = scatter(1, &r, full, &surface, &[]).unwrap();
    let mut split = scatter(1, &r, b, &surface, &[]).unwrap();
    split.extend(scatter(1, &r, a, &surface, &[]).unwrap());
    all.sort_by_key(|i| i.identity);
    split.sort_by_key(|i| i.identity);
    assert_eq!(all, split);
    assert_eq!(
        all.len(),
        all.iter()
            .map(|i| i.identity)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    assert_ne!(
        all,
        scatter(1, &Rule { seed: 43, ..r }, full, &surface, &[]).unwrap()
    );
}
#[test]
fn filters_holes_and_exclusion_zones_are_obeyed() {
    let r = rule();
    let b = Bounds {
        min: [0., 0.],
        max: [64., 64.],
    };
    for invalid in [
        SurfaceSample {
            height: 150.,
            normal: [0., 1., 0.],
            weights: [1., 0., 0., 0.],
        },
        SurfaceSample {
            height: 10.,
            normal: [1., 0., 0.],
            weights: [1., 0., 0., 0.],
        },
        SurfaceSample {
            height: 10.,
            normal: [0., 1., 0.],
            weights: [0., 1., 0., 0.],
        },
    ] {
        assert!(
            scatter(1, &r, b, &move |_| Some(invalid), &[])
                .unwrap()
                .is_empty()
        );
    }
    assert!(scatter(1, &r, b, &|_| None, &[]).unwrap().is_empty());
    let zones = [Exclusion {
        center: [32., 32.],
        radius: 20.,
    }];
    let v = scatter(1, &r, b, &surface, &zones).unwrap();
    assert!(!v.is_empty());
    for i in v {
        assert!((i.position_scale[0] - 32.).hypot(i.position_scale[2] - 32.) >= 20.);
    }
    for b in [0., 0.1, 0.5, 0.99, 1.] {
        for n in [0., 0.1, 0.5, 0.99] {
            assert_ne!(lod_visible(n, b, false), lod_visible(n, b, true));
        }
    }
}
#[test]
fn manual_paint_and_rules_use_kernel_undo_and_canonical_files() {
    use ara_cmd::{Batch, Call, Registry, Session};
    use ara_doc::{AssetId, Document, Workspace};
    use ara_ops::{Author, Store};
    fn batch(calls: Vec<Call>) -> Batch {
        let mut batch = Batch::new("fo_grass001", []);
        batch.calls = calls;
        batch
    }
    let mut db = ara_schema::SchemaDb::new();
    ara_foliage::register(&mut db).unwrap();
    let mut reg = Registry::default();
    ara_foliage::commands::register(&mut reg, &mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(reg.check_examples(&db).is_empty());
    let mut ws = Workspace::default();
    ws.insert(
        "grass.foliage",
        Document::new("foliage", AssetId::parse("fo_grass001").unwrap()),
    );
    let mut store = Store::new(db, ws);
    let initial = batch(vec![
        Call {
            command: "foliage.create".into(),
            params: serde_json::json!({"header":{"terrain":"@world.terrain#tr_world001"}}),
        },
        Call {
            command: "foliage.rule.set".into(),
            params: serde_json::json!({"id":"r_grass","rule":rule()}),
        },
    ]);
    reg.apply(&mut store, &initial, &Session::human()).unwrap();
    let mut exclusion = batch(vec![Call {
        command: "foliage.exclude".into(),
        params: serde_json::json!({"id":"x_compound","zone":{"center":[0,0],"radius":4}}),
    }]);
    exclusion.dry_run = true;
    let preview = reg
        .apply(&mut store, &exclusion, &Session::human())
        .unwrap();
    exclusion.dry_run = false;
    let applied = reg
        .apply(&mut store, &exclusion, &Session::human())
        .unwrap();
    assert_eq!(preview.ops, applied.ops);
    assert!(
        store
            .workspace()
            .doc("fo_grass001")
            .unwrap()
            .record("x_compound")
            .is_some()
    );
    let before = ara_doc::write(
        store.workspace().doc("fo_grass001").unwrap(),
        store.schema(),
    );
    reg.apply(&mut store,&batch(vec![Call{command:"foliage.paint".into(),params:serde_json::json!({"stroke":{"rule":"r_grass","seed":42,"points":[[1,2],[3,4]]}})}]),&Session::human()).unwrap();
    let after = ara_doc::write(
        store.workspace().doc("fo_grass001").unwrap(),
        store.schema(),
    );
    assert_ne!(before, after);
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(
            store.workspace().doc("fo_grass001").unwrap(),
            store.schema()
        )
    );
    store.redo_last(&Author::Human).unwrap();
    assert_eq!(
        after,
        ara_doc::write(
            store.workspace().doc("fo_grass001").unwrap(),
            store.schema()
        )
    );
    let doc = ara_doc::read(&after, "grass.foliage", store.schema(), None)
        .doc
        .unwrap();
    let r: Rule = serde_json::from_value(serde_json::Value::Object(
        doc.record("r_grass").unwrap().clone(),
    ))
    .unwrap();
    assert_eq!(r, rule());
}
