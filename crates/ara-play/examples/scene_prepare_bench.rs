//! CPU-only Play transfer and actual scene projection; no GPU or C# dependencies.
//! --rows 10000|100000 --samples 5 --mode borrowed|owned. Run the same binary in
//! fresh processes in both orders; allocation counts use a separate untimed run.
#[path = "../tests/support/scene.rs"]
mod fixture;
use ara_play::PlayScene;
use ara_schema::SchemaDb;
use ara_sim::{EditWorld, Session, Snapshot, StateHash, TickConfig};
use serde::Serialize;
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

struct Allocations;
static TRACK: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: all allocation operations are delegated unchanged to the system allocator.
unsafe impl GlobalAlloc for Allocations {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the pointer/layout contract is forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
        }
        // SAFETY: the pointer/layout/new-size contract is forwarded unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocations = Allocations;

#[derive(Serialize)]
struct Sample {
    decode_ms: f64,
    workspace_ms: f64,
    projection_ms: f64,
    decoded_scene_drop_ms: f64,
    total_ms: f64,
}

fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn prepare(bytes: &[u8], schema: &SchemaDb, owned: bool) -> (EditWorld, Sample) {
    let total = Instant::now();
    let start = Instant::now();
    let mut scene = PlayScene::decode(bytes).expect("decode");
    let decode_ms = elapsed(start);
    let start = Instant::now();
    let (workspace, id) = if owned {
        scene.take_workspace()
    } else {
        fixture::borrowed_workspace(&scene)
    }
    .expect("workspace");
    let workspace_ms = elapsed(start);
    let start = Instant::now();
    let edit = fixture::project(schema, workspace, id);
    let projection_ms = elapsed(start);
    let start = Instant::now();
    drop(scene);
    let decoded_scene_drop_ms = elapsed(start);
    let sample = Sample {
        decode_ms,
        workspace_ms,
        projection_ms,
        decoded_scene_drop_ms,
        total_ms: elapsed(total),
    };
    (edit, sample)
}

fn number(name: &str, default: usize) -> usize {
    std::env::args()
        .skip_while(|a| a != name)
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let rows = number("--rows", 10000).max(17);
    let samples = number("--samples", 5).max(1);
    let mode = std::env::args()
        .skip_while(|a| a != "--mode")
        .nth(1)
        .unwrap_or_else(|| "owned".into());
    assert!(
        matches!(mode.as_str(), "owned" | "borrowed"),
        "--mode owned|borrowed"
    );
    let owned = mode == "owned";
    let schema = fixture::schema();
    let source = fixture::source(rows);
    let start = Instant::now();
    let scene = fixture::snapshot(&source);
    let capture_ms = elapsed(start);
    let start = Instant::now();
    let bytes = scene.encode().expect("encode");
    let encode_ms = elapsed(start);
    drop(scene);
    drop(prepare(&bytes, &schema, owned));
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let (edit, sample) = prepare(&bytes, &schema, owned);
        timings.push(sample);
        drop(edit);
    }
    COUNT.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    TRACK.store(true, Ordering::Relaxed);
    let (mut edit, _) = prepare(&bytes, &schema, owned);
    TRACK.store(false, Ordering::Relaxed);
    let allocations = COUNT.load(Ordering::Relaxed);
    let allocated_bytes = BYTES.load(Ordering::Relaxed);
    // Authoring mirrors and every projected field are checked outside timing.
    for (path, doc) in source.docs() {
        assert_eq!(edit.workspace().path_of(doc.id.as_str()), Some(path));
        assert_eq!(
            ara_doc::write(doc, &schema),
            ara_doc::write(
                edit.workspace().doc(doc.id.as_str()).expect("mirror"),
                &schema
            )
        );
    }
    let handles = edit
        .world()
        .iter_entities()
        .filter_map(|e| Some((e.get::<ara_sim::Identity>()?.0.clone(), e.id())))
        .collect::<Vec<_>>();
    assert_eq!(
        handles.len(),
        rows + (rows - 1) / 16,
        "prefab children included"
    );
    let make_session = |edit| {
        Session::local(
            edit,
            TickConfig::default(),
            901,
            ara_sim::input::ActionMap::default(),
        )
        .expect("session")
    };
    // Snapshot round trips retain exact runtime numbers and mirrored source defaults.
    let build = StateHash::of_bytes(b"scene-preparation-bench");
    let mut session = make_session(edit);
    let initial_hash = session.state_hash().expect("initial state");
    let snapshot = session.snapshot(build).expect("snapshot");
    let portable = snapshot.to_bytes().expect("portable encode");
    let restored = Snapshot::from_bytes(&portable).expect("portable decode");
    session.restore(&restored, build).expect("portable restore");
    assert_eq!(session.state_hash().expect("restored state"), initial_hash);
    // Restore can recreate storage, so test live-edit handle reuse on its own fresh
    // projection rather than confusing stable identity with a saved raw handle.
    let portable_bytes = portable.len();
    let portable_hash = StateHash::of_bytes(&portable);
    drop(snapshot);
    drop(restored);
    drop(portable);
    drop(handles);
    drop(session);
    let (fresh, _) = prepare(&bytes, &schema, owned);
    edit = fresh;
    let handles = edit
        .world()
        .iter_entities()
        .filter_map(|e| Some((e.get::<ara_sim::Identity>()?.0.clone(), e.id())))
        .collect::<Vec<_>>();
    let change = Instant::now();
    fixture::change(&mut edit);
    let one_change_ms = elapsed(change);
    for (id, entity) in handles {
        assert_eq!(edit.entity(&id), Some(entity), "unchanged identity {id}");
    }
    assert_eq!(
        edit.workspace()
            .doc("sc_bench")
            .expect("scene")
            .record("e_00000000")
            .expect("record")["Stats"]["hp"],
        json!(73)
    );
    let session = make_session(edit);
    let changed_hash = session.state_hash().expect("changed state");
    let changed_snapshot = session.snapshot(build).expect("changed snapshot");
    let mut sorted = timings.iter().map(|s| s.total_ms).collect::<Vec<_>>();
    sorted.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"mode":mode,"rows":rows,"projected_entities":rows+(rows-1)/16,
        "samples":samples,"warmup":1,"capture_ms":capture_ms,"encode_ms":encode_ms,
        "cbor_bytes":bytes.len(),"p50_ms":sorted[samples/2],"p95_ms":sorted[(samples*95/100).min(samples-1)],
        "phases":timings,"rust_allocations_per_preparation":allocations,
        "rust_allocated_bytes_per_preparation":allocated_bytes,"initial_state_hash":initial_hash.to_string(),
        "changed_state_hash":changed_hash.to_string(),"portable_snapshot_hash":portable_hash.to_string(),
        "changed_portable_snapshot_hash":StateHash::of_bytes(&changed_snapshot.to_bytes().expect("changed portable")).to_string(),
        "portable_bytes":portable_bytes,
        "one_change_ms":one_change_ms,"all_live_entity_handles_retained":true,
        "canonical_source_mirror_equal":true,"portable_snapshot_restore_equal":true})
    );
}
