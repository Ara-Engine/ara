#![allow(clippy::unwrap_used)]
use ara_asset::FileCache;
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_schema::SchemaDb;
use ara_sim::{EditWorld, Session, SimRegistry, TickConfig};
use ara_world::*;
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[derive(Default)]
struct Events {
    ready: Vec<String>,
    removed: Vec<String>,
    visibility: Vec<(String, bool)>,
}
impl Interest for Events {
    fn ready(&mut self, cell: &str, _: CellKey, _: bool, _: &[HlodProxy]) {
        self.ready.push(cell.into());
    }
    fn removed(&mut self, cell: &str, _: CellKey) {
        self.removed.push(cell.into());
    }
    fn visibility(&mut self, cell: &str, visible: bool) {
        self.visibility.push((cell.into(), visible));
    }
}

#[test]
fn async_cached_text_activation_layers_grace_memory_and_unload_are_bounded() {
    let mut db = SchemaDb::new();
    register(&mut db);
    ara_components::register(&mut db);
    let registry = SimRegistry::new(db.clone()).unwrap();
    let mut root = Document::new("world", AssetId::parse("wd_test").unwrap());
    for (id, value) in [
        ("w_grid", json!({"cellSize":250})),
        (
            "l_art",
            json!({"name":"Art","loaded":true,"visible":true,"range":0,"graceTicks":3}),
        ),
        (
            "c_a",
            json!({"x":0,"z":0,"slot":0,"layer":"l_art","documents":["@a.scene#sc_a"]}),
        ),
        (
            "c_b",
            json!({"x":1,"z":0,"slot":0,"layer":"l_art","documents":["@b.scene#sc_b"]}),
        ),
    ] {
        root.insert_unchecked(
            RecordId::parse(id).unwrap(),
            serde_json::from_value(value).unwrap(),
        );
    }
    let manifest = Manifest::read(&root).unwrap();
    let mut workspace = Workspace::default();
    for (id, file, cell) in [("sc_a", "a.scene", 0), ("sc_b", "b.scene", 1)] {
        let mut scene = Document::new("scene", AssetId::parse(id).unwrap());
        for i in 0..20 {
            scene.insert_unchecked(
                RecordId::parse(&format!("e_{}", cell * 100 + i)).unwrap(),
                serde_json::from_value(
                    json!({"Transform":{"position":[cell as f32*250.0+i as f32,0,0]}}),
                )
                .unwrap(),
            );
        }
        workspace.insert(file, scene);
    }
    let dir = tempfile::tempdir().unwrap();
    for (path, doc) in workspace.docs() {
        std::fs::write(dir.path().join(path), ara_doc::write(doc, &db)).unwrap();
    }
    let cache = Arc::new(FileCache::new(dir.path()).unwrap());
    let source = Arc::new(
        EditorSource::new(
            dir.path(),
            Manifest::read(&root).unwrap(),
            db.clone(),
            cache.clone(),
        )
        .unwrap(),
    );
    let first = source.load("c_a", 1024 * 1024).unwrap().encode().unwrap();
    assert_eq!(
        first,
        source.load("c_a", 1024 * 1024).unwrap().encode().unwrap()
    );
    let pool = WorkerPool::new(source, registry.clone(), 2, 1024 * 1024).unwrap();
    let mut streaming = Streaming::new(
        &manifest,
        pool,
        StreamConfig {
            memory_bytes: 1024 * 1024,
            cell_reservation: 1024 * 1024,
            lookahead_seconds: 0.0,
        },
    )
    .unwrap();
    let edit = EditWorld::new(
        registry,
        Document::new("scene", AssetId::parse("sc_empty").unwrap()),
    )
    .unwrap();
    let mut session = Session::local(edit, TickConfig::default(), 1, Default::default()).unwrap();
    let source = StreamingSource {
        position: [100.0, 0.0, 100.0],
        velocity: [0.0; 3],
        range_scale: 1.0,
    };
    let budget = ActivationBudget {
        time: Duration::from_secs(1),
        max_entities: 3,
    };
    let mut events = Events::default();
    let mut tick = 0;
    for _ in 0..1000 {
        tick += 1;
        streaming
            .update(&manifest, &[source], tick, &mut events)
            .unwrap();
        let mut stats = FrameStats::default();
        session
            .step_with_streaming(|boundary| {
                stats = streaming.activate(boundary, budget, &mut events).unwrap();
                Ok(())
            })
            .unwrap();
        assert!(stats.spawned <= 3);
        assert!(stats.reserved_bytes <= 1024 * 1024);
        if streaming.is_resident("c_a") {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(streaming.is_resident("c_a"));
    assert_eq!(streaming.entity_count(), 20);
    assert_eq!(events.ready, ["c_a"]);
    let mut hidden = Manifest::read(&root).unwrap();
    hidden.layers.get_mut("l_art").unwrap().visible = false;
    tick += 1;
    streaming
        .update(&hidden, &[source], tick, &mut events)
        .unwrap();
    assert_eq!(events.visibility, [("c_a".into(), false)]);
    assert_eq!(streaming.entity_count(), 20);
    for _ in 0..2 {
        tick += 1;
        streaming.update(&hidden, &[], tick, &mut events).unwrap();
        session
            .step_with_streaming(|boundary| {
                assert_eq!(
                    streaming
                        .activate(boundary, budget, &mut events)
                        .unwrap()
                        .despawned,
                    0
                );
                Ok(())
            })
            .unwrap();
    }
    // Both cells remain wanted; the one-cell budget must replace the farther
    // cell with the nearer one, rather than wait forever for free memory.
    hidden.layers.get_mut("l_art").unwrap().range = 500.0;
    let near_b = StreamingSource {
        position: [350.0, 0.0, 100.0],
        ..source
    };
    for _ in 0..1000 {
        tick += 1;
        streaming
            .update(&hidden, &[near_b], tick, &mut events)
            .unwrap();
        session
            .step_with_streaming(|boundary| {
                let stats = streaming.activate(boundary, budget, &mut events).unwrap();
                assert!(stats.spawned + stats.despawned <= 3);
                assert!(stats.reserved_bytes <= 1024 * 1024);
                Ok(())
            })
            .unwrap();
        if streaming.is_resident("c_b") {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(streaming.is_resident("c_b"));
    assert!(!streaming.is_resident("c_a"));
    assert_eq!(
        streaming
            .relevant_cells(&hidden, near_b)
            .unwrap()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        ["c_b"]
    );
    assert_eq!(events.ready, ["c_a", "c_b"]);
    hidden.layers.get_mut("l_art").unwrap().loaded = false;
    for _ in 0..1000 {
        tick += 1;
        streaming
            .update(&hidden, &[source], tick, &mut events)
            .unwrap();
        session
            .step_with_streaming(|boundary| {
                let stats = streaming.activate(boundary, budget, &mut events).unwrap();
                assert!(stats.despawned <= 3);
                Ok(())
            })
            .unwrap();
        if streaming.reserved_bytes() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(streaming.entity_count(), 0);
    assert_eq!(streaming.reserved_bytes(), 0);
    assert_eq!(events.removed, ["c_a", "c_b"]);
}

/// Cooks cells from an in-memory workspace, slowly (a busy worker machine).
struct SlowSource {
    manifest: Manifest,
    workspace: Workspace,
    db: SchemaDb,
}
impl CellSource for SlowSource {
    fn load(&self, cell: &str, _: usize) -> ara_world::Result<CellBlob> {
        std::thread::sleep(Duration::from_millis(40));
        cook_cell(&self.manifest, cell, &self.workspace, &self.db, None)
    }
}

/// Two 4-row cells side by side (32 m), cooked slowly from memory; one session.
fn slow_world() -> (Manifest, Arc<SlowSource>, SimRegistry, Session) {
    let mut db = SchemaDb::new();
    register(&mut db);
    ara_components::register(&mut db);
    let registry = SimRegistry::new(db.clone()).unwrap();
    let mut root = Document::new("world", AssetId::parse("wd_test").unwrap());
    for (id, value) in [
        ("w_grid", json!({"cellSize":32})),
        (
            "l_art",
            json!({"name":"Art","loaded":true,"visible":true,"range":4,"graceTicks":3}),
        ),
        (
            "c_a",
            json!({"x":0,"z":0,"slot":0,"layer":"l_art","documents":["@a.scene#sc_a"]}),
        ),
        (
            "c_b",
            json!({"x":1,"z":0,"slot":0,"layer":"l_art","documents":["@b.scene#sc_b"]}),
        ),
    ] {
        root.insert_unchecked(
            RecordId::parse(id).unwrap(),
            serde_json::from_value(value).unwrap(),
        );
    }
    let manifest = Manifest::read(&root).unwrap();
    let mut workspace = Workspace::default();
    for (id, file, cell) in [("sc_a", "a.scene", 0), ("sc_b", "b.scene", 1)] {
        let mut scene = Document::new("scene", AssetId::parse(id).unwrap());
        for i in 0..4 {
            scene.insert_unchecked(
                RecordId::parse(&format!("e_{}", cell * 100 + i)).unwrap(),
                serde_json::from_value(
                    json!({"Transform":{"position":[cell as f32*32.0+i as f32,0,0]}}),
                )
                .unwrap(),
            );
        }
        workspace.insert(file, scene);
    }
    let source = Arc::new(SlowSource {
        manifest: manifest.clone(),
        workspace,
        db: db.clone(),
    });
    let edit = EditWorld::new(
        registry.clone(),
        Document::new("scene", AssetId::parse("sc_empty").unwrap()),
    )
    .unwrap();
    let session = Session::local(edit, TickConfig::default(), 1, Default::default()).unwrap();
    (manifest, source, registry, session)
}

fn slow_streaming(
    manifest: &Manifest,
    source: &Arc<SlowSource>,
    registry: &SimRegistry,
) -> Streaming {
    let pool = WorkerPool::new(source.clone(), registry.clone(), 1, 1024 * 1024).unwrap();
    Streaming::new(
        manifest,
        pool,
        StreamConfig {
            memory_bytes: 4 * 1024 * 1024,
            cell_reservation: 1024 * 1024,
            lookahead_seconds: 0.0,
        },
    )
    .unwrap()
}

#[test]
fn settled_streaming_arrives_at_the_first_boundary_whatever_the_worker_speed() {
    let (manifest, source, registry, mut session) = slow_world();
    let mut streaming = slow_streaming(&manifest, &source, &registry);
    // Near the shared edge: both cells are wanted, one worker cooks them in turn.
    let near_both = StreamingSource {
        position: [31.0, 0.0, 16.0],
        velocity: [0.0; 3],
        range_scale: 1.0,
    };
    let budget = ActivationBudget {
        time: Duration::from_secs(1),
        max_entities: 64,
    };
    let mut events = Events::default();
    let settled = streaming
        .settle(
            &manifest,
            &[near_both],
            1,
            &mut events,
            Duration::from_secs(10),
        )
        .unwrap();
    assert!(settled);
    session
        .step_with_streaming(|boundary| {
            streaming.activate(boundary, budget, &mut events).unwrap();
            Ok(())
        })
        .unwrap();
    assert!(streaming.is_resident("c_a") && streaming.is_resident("c_b"));
    assert_eq!(events.ready, ["c_a", "c_b"]);
    assert_eq!(streaming.entity_count(), 8);
}

#[test]
fn fresh_state_adopts_rows_a_restored_world_already_holds() {
    let (manifest, source, registry, mut session) = slow_world();
    let near_a = StreamingSource {
        position: [10.0, 0.0, 16.0],
        velocity: [0.0; 3],
        range_scale: 1.0,
    };
    let budget = ActivationBudget {
        time: Duration::from_secs(1),
        max_entities: 64,
    };
    let mut events = Events::default();
    let mut step = |streaming: &mut Streaming, session: &mut Session, tick| {
        streaming
            .settle(
                &manifest,
                &[near_a],
                tick,
                &mut events,
                Duration::from_secs(10),
            )
            .unwrap();
        let mut stats = FrameStats::default();
        let result = session.step_with_streaming(|boundary| {
            stats = streaming
                .activate(boundary, budget, &mut Events::default())
                .map_err(|e| ara_sim::SimError(e.to_string()))?;
            Ok(())
        });
        (result.map(drop), stats)
    };
    let mut first = slow_streaming(&manifest, &source, &registry);
    step(&mut first, &mut session, 1).0.unwrap();
    assert!(first.is_resident("c_a"));
    // A restored world holds c_a's rows; a fresh state would spawn them again.
    let mut fresh = slow_streaming(&manifest, &source, &registry);
    let duplicate = step(&mut fresh, &mut session, 1).0;
    assert!(
        duplicate.is_err(),
        "duplicate rows must fail without adoption"
    );
    let (_, _, _, mut restored) = slow_world();
    step(
        &mut slow_streaming(&manifest, &source, &registry),
        &mut restored,
        1,
    )
    .0
    .unwrap();
    let mut adopting = slow_streaming(&manifest, &source, &registry);
    adopting.adopt_resident(true);
    let (result, stats) = step(&mut adopting, &mut restored, 2);
    result.unwrap();
    assert!(adopting.is_resident("c_a"));
    assert_eq!(stats.spawned, 0, "adopted rows are not spawned again");
    assert_eq!(adopting.entity_count(), 4);
}
