use ara_doc::refs::AssetIndex;
use ara_material::{Material, dsl, register};
use ara_schema::SchemaDb;
use std::path::PathBuf;

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
pub fn assets() -> AssetIndex {
    let mut assets = AssetIndex::new();
    for (id, path) in [
        ("tx_paint001", "paint.png"),
        ("tx_leaf001", "leaf.png"),
        ("tx_leafmask1", "leaf-mask.png"),
        ("tx_screen01", "screen.png"),
        ("tx_grass001", "grass.png"),
        ("tx_rock0001", "rock.png"),
    ] {
        assets.insert(id, format!("textures/{path}"), None);
    }
    assets
}
pub fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    register(&mut db).expect("material schemas register");
    db
}
pub fn example(name: &str) -> Material {
    let id = match name {
        "painted-metal" => "gr_painted",
        "car-paint" => "gr_carpaint",
        "foliage" => "gr_foliage",
        "emissive-screen" => "gr_screen",
        "ui-rounded-panel" => "gr_panel",
        "terrain-blend" => "gr_terrain",
        _ => panic!("unknown fixture"),
    };
    let source = std::fs::read_to_string(root().join(format!("examples/{name}.material")))
        .expect("example text");
    let sidecar = root().join(format!("examples/{name}.graph"));
    let mut m = if sidecar.exists() {
        let graph = ara_graph::Graph::read(
            &std::fs::read_to_string(sidecar).expect("graph sidecar"),
            &schema(),
        )
        .expect("canonical graph storage");
        assert_eq!(graph.id.as_str(), id);
        dsl::reconcile_graph(&source, &graph)
            .expect("existing names reconcile")
            .material
    } else {
        dsl::parse(&source, id).expect("new authored DSL parses")
    };
    m.resolve_assets(&assets())
        .expect("example references resolve");
    m
}
