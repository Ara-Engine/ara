#![allow(clippy::unwrap_used)]
use ara_2d::{
    Flipbook, FlipbookFrame, GridLayout, Tile, TileDefinition, TileLayer, Tilemap, tilemap,
};
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;

fn store() -> (Store, Registry) {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let mut registry = Registry::builtin(&mut db);
    ara_2d::commands::register(&mut registry, &mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(registry.check_examples(&db).is_empty());
    let mut workspace = Workspace::default();
    workspace.insert(
        "test.scene",
        Document::new("scene", AssetId::parse("sc_test1234").unwrap()),
    );
    (Store::new(db, workspace), registry)
}
fn call(command: &'static str, params: serde_json::Value) -> (&'static str, serde_json::Value) {
    (command, params)
}
fn map() -> Tilemap {
    Tilemap {
        definitions: [
            (
                "1".into(),
                TileDefinition {
                    region: "grass".into(),
                    solid: true,
                    lit: false,
                },
            ),
            (
                "2".into(),
                TileDefinition {
                    region: "edge".into(),
                    solid: true,
                    lit: false,
                },
            ),
        ]
        .into(),
        layers: vec![TileLayer {
            id: "l_ground".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}
#[test]
fn strokes_fill_rules_and_erase_are_atomic_and_undoable() {
    let (mut store, reg) = store();
    let session = Session::human();
    reg.apply(
        &mut store,
        &Batch::new(
            "sc_test1234",
            vec![
                call("entity.create", json!({"id":"e_map1","name":"Map"})),
                call("tilemap.set", json!({"entity":"e_map1","value":map()})),
            ],
        ),
        &session,
    )
    .unwrap();
    let before = ara_doc::write(
        store.workspace().doc("sc_test1234").unwrap(),
        store.schema(),
    );
    let result=reg.apply(&mut store,&Batch::new("sc_test1234",vec![call("tilemap.paint",json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":-1,"y":0,"tile":1,"terrain":1},{"x":0,"y":0,"tile":1,"terrain":1}]})),call("tilemap.paint",json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":1,"y":0,"tile":2,"terrain":1}]}))]),&session).unwrap();
    assert_eq!(result.ops.len(), 3);
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(
            store.workspace().doc("sc_test1234").unwrap(),
            store.schema()
        )
    );
    store.redo_last(&Author::Human).unwrap();
    let after = ara_doc::write(
        store.workspace().doc("sc_test1234").unwrap(),
        store.schema(),
    );
    assert!(reg.apply(&mut store,&Batch::new("sc_test1234",vec![call("tilemap.paint",json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":8,"y":0,"tile":1,"terrain":1}]})),call("tilemap.paint",json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":9,"y":0,"tile":99,"terrain":1}]}))]),&session).is_err());
    assert_eq!(
        after,
        ara_doc::write(
            store.workspace().doc("sc_test1234").unwrap(),
            store.schema()
        )
    );
    reg.apply(&mut store,&Batch::new("sc_test1234",vec![call("tilemap.fill",json!({"entity":"e_map1","layer":"l_ground","seed":{"x":0,"y":1},"min":{"x":-2,"y":-1},"max":{"x":2,"y":1},"tile":1,"terrain":2})),call("tilemap.erase",json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":0,"y":0}]})),call("tilemap.rule.apply",json!({"entity":"e_map1","layer":"l_ground"}))]),&session).unwrap();
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        after,
        ara_doc::write(
            store.workspace().doc("sc_test1234").unwrap(),
            store.schema()
        )
    );
}
#[test]
fn cache_chunks_and_collision_silhouettes_cover_signed_layouts() {
    let mut map = map();
    for x in -2..2 {
        for y in -1..1 {
            map.layers[0].cells.insert(
                tilemap::key(x, y),
                Tile {
                    tile: 1,
                    terrain: 1,
                },
            );
        }
    }
    let chunks = tilemap::chunks(&map).unwrap();
    assert_eq!(chunks.len(), 4);
    assert_eq!(chunks, tilemap::chunks(&map).unwrap());
    assert_eq!(tilemap::colliders(&map).unwrap().len(), 1);
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
}
#[test]
fn rules_read_one_source_and_flipbooks_use_fixed_ticks() {
    let mut map = map();
    map.rules = vec![ara_2d::TileRule {
        terrain: 1,
        required: 1,
        forbidden: 0,
        tile: 2,
    }];
    for x in 0..3 {
        map.layers[0].cells.insert(
            tilemap::key(x, 0),
            Tile {
                tile: 1,
                terrain: 1,
            },
        );
    }
    let changes = tilemap::apply_rules(&map, &map.layers[0]).unwrap();
    assert_eq!(changes.len(), 2);
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
