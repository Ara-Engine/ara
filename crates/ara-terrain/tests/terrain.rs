#![allow(clippy::unwrap_used)]
use ara_cmd::{Batch, Call, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use ara_terrain::{
    Terrain, Tile, TileCoord,
    cache::{MemoryCache, TileSource},
    commands, lod,
    stream::Residency,
};
use serde_json::{Value, json};

fn batch(calls: Vec<Call>) -> Batch {
    let mut batch = Batch::new("tr_world001", []);
    batch.calls = calls;
    batch
}
fn setup() -> (Store, Registry) {
    let mut db = SchemaDb::new();
    ara_terrain::register(&mut db).unwrap();
    let mut reg = Registry::default();
    commands::register(&mut reg, &mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(reg.check_examples(&db).is_empty());
    let mut ws = Workspace::default();
    ws.insert(
        "world.terrain",
        Document::new("terrain", AssetId::parse("tr_world001").unwrap()),
    );
    (Store::new(db, ws), reg)
}
fn apply(s: &mut Store, r: &Registry, command: &str, params: Value) -> ara_cmd::BatchResult {
    r.apply(
        s,
        &batch(vec![Call {
            command: command.into(),
            params,
        }]),
        &Session::human(),
    )
    .unwrap()
}
fn init(s: &mut Store, r: &Registry) {
    apply(
        s,
        r,
        "terrain.create",
        json!({"grid":{"origin":[0,0],"spacing":1,"cells":16,"min":{"x":-1,"z":-1},"max":{"x":2,"z":2},"materials":[]}}),
    );
    apply(
        s,
        r,
        "terrain.layer.add",
        json!({"id":"l_base","layer":{"name":"Sculpt","priority":0,"enabled":true,"opacity":1}}),
    );
}
fn terrain(s: &Store) -> Terrain {
    Terrain::from_document(s.workspace().doc("tr_world001").unwrap()).unwrap()
}

#[test]
fn explicit_stroke_ids_make_independent_rebuilds_identical() {
    let build = || {
        let (mut store, registry) = setup();
        init(&mut store, &registry);
        for (command, id) in [("terrain.raise", "s_hill"), ("terrain.paint", "s_soil")] {
            apply(
                &mut store,
                &registry,
                command,
                json!({"id":id,"layer":"l_base","dabs":[{"center":[0,0],"radius":4,"amount":1}]}),
            );
        }
        let t = terrain(&store);
        assert!(t.strokes.contains_key("s_hill"));
        assert!(t.tiles.contains_key("t_n1zn1"));
        ara_doc::write(
            store.workspace().doc("tr_world001").unwrap(),
            store.schema(),
        )
    };
    assert_eq!(build(), build());
}
#[test]
fn gesture_dryrun_atomic_undo_redo_and_canonical_roundtrip() {
    let (mut s, r) = setup();
    init(&mut s, &r);
    let before = terrain(&s);
    let mut batch = batch(vec![Call {
        command: "terrain.raise".into(),
        params: json!({"layer":"l_base","dabs":[{"center":[16,8],"radius":6,"amount":4},{"center":[17,9],"radius":3,"amount":1}]}),
    }]);
    batch.dry_run = true;
    let preview = r.apply(&mut s, &batch, &Session::human()).unwrap();
    assert_eq!(terrain(&s), before);
    batch.dry_run = false;
    let result = r.apply(&mut s, &batch, &Session::human()).unwrap();
    assert_eq!(result.ops, preview.ops);
    let after = terrain(&s);
    assert_eq!(after.strokes.len(), 1);
    assert_eq!(after.tiles.len(), 2);
    s.undo_last(&Author::Human).unwrap();
    assert_eq!(terrain(&s), before);
    s.redo_last(&Author::Human).unwrap();
    assert_eq!(terrain(&s), after);
    let text = ara_doc::write(s.workspace().doc("tr_world001").unwrap(), s.schema());
    let read = ara_doc::read(&text, "world.terrain", s.schema(), None);
    assert!(!read.diags.iter().any(|d| d.is_error()), "{:?}", read.diags);
    let doc = read.doc.unwrap();
    assert_eq!(Terrain::from_document(&doc).unwrap(), after);
    assert_eq!(ara_doc::write(&doc, s.schema()), text);
    let revision = s.revision();
    let mut failed = Batch::new("tr_world001", []);
    failed.calls = vec![
        Call {
            command: "terrain.raise".into(),
            params: batch.calls[0].params.clone(),
        },
        Call {
            command: "terrain.paint".into(),
            params: json!({"layer":"l_missing","dabs":[{"center":[0,0],"radius":1,"amount":1}]}),
        },
    ];
    assert!(r.apply(&mut s, &failed, &Session::human()).is_err());
    assert_eq!(s.revision(), revision);
    assert_eq!(terrain(&s), after);
}
#[test]
fn layers_stamp_paint_holes_erosion_and_independent_tile_edges() {
    let (mut s, r) = setup();
    init(&mut s, &r);
    let mut cache = MemoryCache::default();
    let mut stamp = Tile::flat(16).unwrap();
    stamp.heights.fill(2.);
    let hash = cache.put(&stamp).unwrap();
    for (command, extra) in [
        ("terrain.raise", json!({})),
        ("terrain.stamp", json!({"stamp":hash,"angle":0.37})),
        ("terrain.paint", json!({"channel":1})),
        ("terrain.erode", json!({"iterations":8})),
        ("terrain.hole", json!({})),
    ] {
        let mut params = json!({"layer":"l_base","dabs":[{"center":[16,8],"radius":7,"amount":if command=="terrain.raise"{4.}else{0.8}}]});
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        apply(&mut s, &r, command, params);
    }
    let t = terrain(&s);
    let a = t.evaluate(TileCoord { x: 0, z: 0 }, &cache).unwrap();
    let b = t.evaluate(TileCoord { x: 1, z: 0 }, &cache).unwrap();
    for z in 0..17 {
        assert_eq!(a.heights[z * 17 + 16], b.heights[z * 17]);
        assert_eq!(a.weights[z * 17 + 16], b.weights[z * 17]);
        assert_eq!(a.holes[z * 17 + 16], b.holes[z * 17]);
    }
    assert!(a.holes.contains(&1));
    assert!(a.weights.iter().any(|w| w[1] > 0));
    assert_eq!(a, t.evaluate(TileCoord { x: 0, z: 0 }, &cache).unwrap());
    let bytes = a.encode().unwrap();
    assert_eq!(Tile::decode(&bytes).unwrap(), a);
    let mut bad = bytes;
    bad.push(0);
    assert!(Tile::decode(&bad).is_err());
    let field = a.heightfield(&t.header, TileCoord { x: 0, z: 0 });
    let mesh = field.mesh();
    assert!(mesh.triangles.len() < 16 * 16 * 2);
    let input = ara_nav::Geometry {
        areas: vec![0; mesh.triangles.len()],
        vertices: mesh.vertices,
        triangles: mesh.triangles,
    };
    assert_eq!(input.areas.len(), input.triangles.len());
    for tri in &input.triangles {
        let [a, b, c] = tri.map(|i| input.vertices[i as usize]);
        let up = (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]);
        assert!(up > 0.);
    }
    apply(
        &mut s,
        &r,
        "terrain.import",
        json!({"layer":"l_base","coord":{"x":0,"z":0},"hash":hash}),
    );
    assert_eq!(
        terrain(&s)
            .evaluate(TileCoord { x: 0, z: 0 }, &cache)
            .unwrap()
            .heights[8 * 17 + 8],
        2.
    );
    s.undo_last(&Author::Human).unwrap();
    assert_eq!(
        terrain(&s)
            .evaluate(TileCoord { x: 0, z: 0 }, &cache)
            .unwrap(),
        a
    );
    s.redo_last(&Author::Human).unwrap();
    apply(
        &mut s,
        &r,
        "terrain.layer.set",
        json!({"id":"l_base","layer":{"name":"Sculpt","priority":0,"enabled":false,"opacity":1}}),
    );
    assert!(
        terrain(&s)
            .evaluate(TileCoord { x: 0, z: 0 }, &cache)
            .unwrap()
            .heights
            .iter()
            .all(|h| *h == 0.)
    );
    assert_eq!(cache.load(&hash).unwrap(), stamp);
}
#[test]
fn cdlod_edges_have_identical_segments_at_every_level_gap() {
    for fine in 0..=5 {
        for coarse in fine..=5 {
            let edge: Vec<_> = (0..=32)
                .step_by(1 << fine)
                .map(|s| lod::stitch_edge(s, fine, coarse))
                .collect();
            let mut unique = edge;
            unique.dedup();
            assert_eq!(unique, (0..=32).step_by(1 << coarse).collect::<Vec<_>>());
        }
    }
    let settings = lod::LodSettings {
        width: 64.,
        distance: 100.,
        max_level: 5,
    };
    assert_eq!(
        settings
            .level([32., 32.], [0., 0.], TileCoord { x: 0, z: 0 })
            .unwrap(),
        0
    );
}
#[test]
fn publication_rejects_obsolete_and_forbidden_tiles_preserving_last_good() {
    let tile = Tile::flat(16).unwrap();
    let mut resident = Residency::new(tile.heights.len() * 9);
    let a = TileCoord { x: 0, z: 0 };
    let b = TileCoord { x: 1, z: 0 };
    resident.request(a, 1);
    assert!(resident.publish(a, 1, tile.clone(), true).unwrap());
    resident.request(a, 2);
    assert!(!resident.publish(a, 1, tile.clone(), true).unwrap());
    assert!(!resident.publish(a, 2, tile.clone(), false).unwrap());
    assert_eq!(resident.get(a).unwrap().1, 1);
    resident.request(b, 1);
    assert!(resident.publish(b, 1, tile, true).unwrap());
    assert!(resident.get(a).is_none());
    assert!(resident.bytes() <= 17 * 17 * 9);
}
