#![allow(clippy::unwrap_used)]
use ara_2d::{
    Flipbook, FlipbookFrame, GridLayout, TileDefinition, TileLayer, TileRule, Tilemap, tilemap,
};
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

fn store() -> (Store, Registry) {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut registry = Registry::builtin(&mut db);
    ara_2d::commands::register(&mut registry, &mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    let problems = registry.check_examples(&db);
    assert!(problems.is_empty(), "{problems:?}");
    let mut workspace = Workspace::default();
    workspace.insert(
        "test.scene",
        Document::new("scene", AssetId::parse("sc_test1234").unwrap()),
    );
    (Store::new(db, workspace), registry)
}
fn text(store: &Store) -> String {
    ara_doc::write(
        store.workspace().doc("sc_test1234").unwrap(),
        store.schema(),
    )
}
fn apply(
    store: &mut Store,
    reg: &Registry,
    calls: Vec<(&'static str, Value)>,
) -> anyhow::Result<usize> {
    reg.apply(store, &Batch::new("sc_test1234", calls), &Session::human())
        .map(|result| result.ops.len())
        .map_err(|e| anyhow::anyhow!("{e:?}"))
}
fn cells(store: &Store) -> Value {
    let doc = store.workspace().doc("sc_test1234").unwrap();
    let record = doc.records().values().next().unwrap();
    record["Tilemap"]["layers"][0]["cells"].clone()
}
/// Terrain 1 = ground: grass (1) where nothing is above, dirt (2) below the surface.
fn map() -> Tilemap {
    let definition = |region: &str, terrain| TileDefinition {
        region: region.into(),
        terrain,
        solid: true,
        lit: false,
    };
    Tilemap {
        definitions: [
            ("1".into(), definition("grass", 1)),
            ("2".into(), definition("dirt", 1)),
            ("3".into(), definition("stone", 0)),
        ]
        .into(),
        layers: vec![TileLayer {
            id: "l_ground".into(),
            ..Default::default()
        }],
        rules: vec![
            TileRule {
                terrain: 1,
                required: 0,
                forbidden: 1 << 2,
                tile: 1,
            },
            TileRule {
                terrain: 1,
                required: 0,
                forbidden: 0,
                tile: 2,
            },
        ],
        ..Default::default()
    }
}
fn setup() -> (Store, Registry) {
    let (mut store, reg) = store();
    apply(
        &mut store,
        &reg,
        vec![
            ("entity.create", json!({"id":"e_map1","name":"Map"})),
            ("tilemap.set", json!({"entity":"e_map1","value":map()})),
        ],
    )
    .unwrap();
    (store, reg)
}

#[test]
fn strokes_are_one_undo_item_and_reject_partial_failure() {
    let (mut store, reg) = setup();
    let before = text(&store);
    let stroke = |x: i32| {
        (
            "tilemap.paint",
            json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":x,"y":0,"tile":3}]}),
        )
    };
    // Three pointer samples of one drag, submitted as one batch.
    assert_eq!(
        apply(&mut store, &reg, vec![stroke(-1), stroke(0), stroke(1)]).unwrap(),
        3
    );
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(before, text(&store), "one undo reverts the whole stroke");
    store.redo_last(&Author::Human).unwrap();
    let after = text(&store);
    let bad = (
        "tilemap.paint",
        json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":9,"y":0,"tile":99}]}),
    );
    assert!(apply(&mut store, &reg, vec![stroke(8), bad]).is_err());
    assert_eq!(after, text(&store), "a failing sample commits nothing");
    // Repainting identical cells emits no ops.
    assert_eq!(apply(&mut store, &reg, vec![stroke(0)]).unwrap(), 0);
}

#[test]
fn autotile_paint_fill_erase_and_rule_apply_resolve_terrain() {
    let (mut store, reg) = setup();
    let column: Vec<_> = (0..3).map(|y| json!({"x":0,"y":y,"tile":2})).collect();
    apply(
        &mut store,
        &reg,
        vec![(
            "tilemap.paint",
            json!({"entity":"e_map1","layer":"l_ground","cells":column,"autotile":true}),
        )],
    )
    .unwrap();
    assert_eq!(
        cells(&store),
        json!({"0,0":2,"0,1":2,"0,2":1}),
        "top becomes grass"
    );
    // Erasing the top re-tiles its neighbor in the same transaction.
    apply(
        &mut store,
        &reg,
        vec![(
            "tilemap.erase",
            json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":0,"y":2}],"autotile":true}),
        )],
    )
    .unwrap();
    assert_eq!(cells(&store), json!({"0,0":2,"0,1":1}));
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(cells(&store), json!({"0,0":2,"0,1":2,"0,2":1}));
    // A bounded fill of empty space next to the column, auto-tiled as one transaction.
    let changed = apply(&mut store, &reg, vec![("tilemap.fill", json!({"entity":"e_map1","layer":"l_ground","seed":{"x":1,"y":0},"min":{"x":1,"y":0},"max":{"x":2,"y":1},"tile":2,"autotile":true}))]).unwrap();
    assert_eq!(changed, 4);
    assert_eq!(cells(&store)["2,1"], json!(1));
    assert_eq!(cells(&store)["2,0"], json!(2));
    // Plain paint leaves rules unapplied; rule.apply then fixes the whole layer.
    apply(
        &mut store,
        &reg,
        vec![(
            "tilemap.paint",
            json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":5,"y":5,"tile":2}]}),
        )],
    )
    .unwrap();
    assert_eq!(
        apply(
            &mut store,
            &reg,
            vec![(
                "tilemap.rule.apply",
                json!({"entity":"e_map1","layer":"l_ground"})
            )]
        )
        .unwrap(),
        1
    );
    assert_eq!(cells(&store)["5,5"], json!(1));
}

#[test]
fn cache_chunks_and_collision_silhouettes_cover_signed_layouts() {
    let mut map = map();
    for x in -2..2 {
        for y in -1..1 {
            map.layers[0].cells.insert(tilemap::key(x, y), 2);
        }
    }
    let chunks = tilemap::chunks(&map).unwrap();
    assert_eq!(chunks.len(), 4);
    assert_eq!(chunks[0].bytes.len(), 16 + 1024 * 4);
    assert_eq!(chunks, tilemap::chunks(&map).unwrap());
    assert_eq!(
        tilemap::colliders(&map).unwrap().len(),
        1,
        "grid cells merge"
    );
    for layout in [GridLayout::Hex, GridLayout::Isometric] {
        map.layout = layout;
        let colliders = tilemap::colliders(&map).unwrap();
        assert_eq!(colliders.len(), 8);
        assert_eq!(
            colliders[0].vertices.len(),
            if layout == GridLayout::Hex { 6 } else { 4 }
        );
    }
    assert!(tilemap::coordinate("01,0").is_err());
    let text = ara_2d::canonical(&map).unwrap();
    assert_eq!(
        text,
        ara_2d::canonical(&serde_json::from_str::<Tilemap>(&text).unwrap()).unwrap()
    );
    let mut bad = map.clone();
    bad.rules[0].tile = 3;
    assert!(bad.validate().is_err(), "rule output must keep its terrain");
}

#[test]
fn rules_read_one_snapshot_and_flipbooks_use_fixed_ticks() {
    let mut map = map();
    for x in 0..3 {
        map.layers[0].cells.insert(tilemap::key(x, 0), 2);
    }
    map.layers[0].cells.insert(tilemap::key(1, 1), 2);
    let changes = tilemap::apply_rules(&map, &map.layers[0], None).unwrap();
    // (0,0) and (2,0) have nothing above; (1,0) is covered by (1,1), which is exposed.
    assert_eq!(
        changes,
        [("0,0".into(), 1), ("1,1".into(), 1), ("2,0".into(), 1)]
    );
    let clip = Flipbook {
        frames: vec![
            FlipbookFrame {
                region: "a".into(),
                ticks: 2,
            },
            FlipbookFrame {
                region: "b".into(),
                ticks: 3,
            },
        ],
        start_tick: 10,
        looping: true,
    };
    assert_eq!(clip.region_at(11).unwrap(), Some("a"));
    assert_eq!(clip.region_at(12).unwrap(), Some("b"));
    assert_eq!(clip.region_at(15).unwrap(), Some("a"));
}

#[test]
fn shadow_outlines_follow_the_solid_boundary_counter_clockwise() {
    let mut map = map();
    // An L of three grid cells: no interior seams, merged runs, solid on the left.
    for (x, y) in [(0, 0), (1, 0), (0, 1)] {
        map.layers[0].cells.insert(tilemap::key(x, y), 2);
    }
    let edges = tilemap::outline(&map).unwrap();
    assert_eq!(edges.len(), 6, "{edges:?}");
    assert!(
        edges.contains(&[-0.5, -0.5, 1.5, -0.5]),
        "merged bottom run"
    );
    let area: f32 = edges
        .iter()
        .map(|[ax, ay, bx, by]| ax * by - bx * ay)
        .sum::<f32>()
        * 0.5;
    assert_eq!(area, 3., "positive signed area: counter-clockwise");
    // Non-solid tiles cast nothing; hex cells expose only their open sides.
    map.layers[0].cells.insert(tilemap::key(5, 5), 3);
    map.definitions.get_mut("3").unwrap().solid = false;
    assert_eq!(tilemap::outline(&map).unwrap().len(), 6);
    map.layout = GridLayout::Hex;
    map.layers[0].cells.clear();
    map.layers[0].cells.insert(tilemap::key(0, 0), 2);
    assert_eq!(tilemap::outline(&map).unwrap().len(), 6);
    map.layers[0].cells.insert(tilemap::key(1, 0), 2);
    assert_eq!(
        tilemap::outline(&map).unwrap().len(),
        10,
        "shared side removed"
    );
}
