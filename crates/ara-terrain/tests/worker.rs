#![allow(clippy::unwrap_used)]
use ara_terrain::{
    Header, Terrain, TileCoord,
    job::{BakeJob, BakeRequest},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
#[test]
fn real_process_cooks_and_delivers_without_blocking_poll() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-output")
        .join(format!("worker-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = root.join("world.terrain");
    let output = root.join("result.json");
    let _ = std::fs::remove_file(&output);
    let mut db = ara_schema::SchemaDb::new();
    ara_terrain::register(&mut db).unwrap();
    let t = Terrain::new(
        ara_doc::AssetId::parse("tr_world001").unwrap(),
        Header {
            origin: [0., 0.],
            spacing: 1.,
            cells: 16,
            min: TileCoord { x: 0, z: 0 },
            max: TileCoord { x: 1, z: 1 },
            materials: vec![],
        },
    )
    .unwrap();
    std::fs::write(&manifest, ara_doc::write(&t.to_document().unwrap(), &db)).unwrap();
    let mut job = BakeJob::spawn(BakeRequest {
        worker: env!("CARGO_BIN_EXE_ara-terrain-worker").into(),
        manifest,
        cache: root.join("cache"),
        output,
        coord: TileCoord { x: 0, z: 0 },
        revision: 7,
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let result = loop {
        if let Some(result) = job.poll().unwrap() {
            break result;
        }
        assert!(Instant::now() < deadline, "worker timed out");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(result.revision, 7);
    assert!(result.tile.heights.iter().all(|h| *h == 0.));
    assert!(job.poll().unwrap().is_none());
}
